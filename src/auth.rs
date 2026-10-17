//! Optional HTTP basic authentication.
//!
//! Credentials come from `--auth-file` / `STREAGIT_AUTH_FILE` (`user:password`
//! lines, blank lines and `#` comments ignored) and/or the environment
//! variables `STREAGIT_AUTH_USER` + `STREAGIT_AUTH_PASSWORD`, never from a
//! command line value. A password may be given as `sha256:<64 hex digits>`
//! (the SHA-256 of the password). Usernames and passwords are compared as
//! SHA-256 digests in constant time against every configured user, so
//! neither a wrong username nor its length shows in the response time.
//!
//! After `MAX_FAILURES` wrong attempts in a row a client IP is locked out for
//! `LOCKOUT` (429). Requests without credentials do not count.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use base64::Engine;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub const MAX_FAILURES: u32 = 10;
pub const LOCKOUT: Duration = Duration::from_secs(15 * 60);
/// Tracked client addresses; beyond this, entries that are not locked out are dropped.
const MAX_TRACKED: usize = 10_000;

type Digest32 = [u8; 32];

fn sha256(b: &[u8]) -> Digest32 {
    Sha256::digest(b).into()
}

fn parse_secret(s: &str) -> Digest32 {
    if let Some(hex) = s.strip_prefix("sha256:") {
        if hex.len() == 64 && hex.bytes().all(|c| c.is_ascii_hexdigit()) {
            let mut d = [0u8; 32];
            for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
                let s = std::str::from_utf8(chunk).expect("ascii");
                d[i] = u8::from_str_radix(s, 16).expect("hex");
            }
            return d;
        }
    }
    sha256(s.as_bytes())
}

#[derive(Default)]
struct Failures {
    count: u32,
    locked_until: Option<Instant>,
}

/// Outcome of checking a request.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    /// No or wrong credentials: 401.
    Denied,
    /// Too many failures from this address: 429 with the remaining seconds.
    LockedOut(u64),
}

pub struct Auth {
    /// (sha256(user), sha256(password))
    users: Vec<(Digest32, Digest32)>,
    failures: Mutex<HashMap<IpAddr, Failures>>,
}

impl Auth {
    /// Load users from `file` and/or the `user`/`password` pair. `Ok(None)` when
    /// nothing is configured; an error for an unreadable or empty file, a
    /// malformed line or only one of user and password.
    pub fn load(
        file: Option<&Path>,
        user: Option<&str>,
        password: Option<&str>,
    ) -> anyhow::Result<Option<Auth>> {
        let mut users = Vec::new();
        if let Some(path) = file {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("auth file {}", path.display()))?;
            for (n, line) in text.lines().enumerate() {
                let line = line.strip_suffix('\r').unwrap_or(line);
                if line.trim().is_empty() || line.trim_start().starts_with('#') {
                    continue;
                }
                match line.split_once(':') {
                    Some((u, p)) if !u.is_empty() && !p.is_empty() => {
                        users.push((sha256(u.as_bytes()), parse_secret(p)));
                    }
                    _ => bail!(
                        "auth file {}: line {} is not `user:password`",
                        path.display(),
                        n + 1
                    ),
                }
            }
            if users.is_empty() {
                bail!("auth file {} contains no users", path.display());
            }
            warn_if_readable_by_others(path);
        }
        match (user, password) {
            (Some(u), Some(p)) => users.push((sha256(u.as_bytes()), parse_secret(p))),
            (None, None) => {}
            _ => bail!("STREAGIT_AUTH_USER and STREAGIT_AUTH_PASSWORD must be set together"),
        }
        if users.is_empty() {
            return Ok(None);
        }
        Ok(Some(Auth {
            users,
            failures: Mutex::new(HashMap::new()),
        }))
    }

    pub fn user_count(&self) -> usize {
        self.users.len()
    }

    /// Whether an `Authorization` header value carries valid credentials.
    pub fn credentials_valid(&self, header: &[u8]) -> bool {
        let Some(b64) = header
            .get(..6)
            .filter(|s| s.eq_ignore_ascii_case(b"basic "))
            .map(|_| &header[6..])
        else {
            return false;
        };
        let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(b64.trim_ascii()) else {
            return false;
        };
        let (u, p) = match decoded.iter().position(|&c| c == b':') {
            Some(i) => (&decoded[..i], &decoded[i + 1..]),
            None => (&decoded[..], &b""[..]),
        };
        let (hu, hp) = (sha256(u), sha256(p));
        let mut ok = subtle::Choice::from(0);
        for (su, sp) in &self.users {
            ok |= su.ct_eq(&hu) & sp.ct_eq(&hp);
        }
        ok.into()
    }

    /// Check a request from `ip` with the given `Authorization` header.
    pub fn check(&self, ip: Option<IpAddr>, header: Option<&[u8]>) -> Verdict {
        let ip = ip.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        let now = Instant::now();
        {
            let mut f = self.failures.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(e) = f.get(&ip) {
                match e.locked_until {
                    Some(t) if t > now => {
                        return Verdict::LockedOut((t - now).as_secs().max(1));
                    }
                    Some(_) => {
                        f.remove(&ip);
                    }
                    None => {}
                }
            }
        }
        let Some(header) = header else {
            return Verdict::Denied;
        };
        let valid = self.credentials_valid(header);
        let mut f = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        if valid {
            f.remove(&ip);
            return Verdict::Ok;
        }
        if f.len() >= MAX_TRACKED && !f.contains_key(&ip) {
            f.retain(|_, e| e.locked_until.is_some_and(|t| t > now));
            if f.len() >= MAX_TRACKED {
                return Verdict::Denied;
            }
        }
        let e = f.entry(ip).or_default();
        e.count += 1;
        if e.count >= MAX_FAILURES {
            e.count = 0;
            e.locked_until = Some(now + LOCKOUT);
            tracing::warn!(%ip, "too many failed logins; locked out for {}s", LOCKOUT.as_secs());
        }
        Verdict::Denied
    }
}

#[cfg(unix)]
fn warn_if_readable_by_others(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(m) = std::fs::metadata(path) {
        if m.permissions().mode() & 0o077 != 0 {
            tracing::warn!(
                path = %path.display(),
                "auth file is accessible by group/others; chmod 600 recommended"
            );
        }
    }
}

#[cfg(not(unix))]
fn warn_if_readable_by_others(_: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic(user: &str, pass: &str) -> Vec<u8> {
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"))
        )
        .into_bytes()
    }

    #[test]
    fn file_and_env_users() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("users");
        let hash = format!("sha256:{}", {
            let d = sha256(b"s3cret");
            d.iter().map(|b| format!("{b:02x}")).collect::<String>()
        });
        std::fs::write(
            &f,
            format!("# comment\n\nalice:pa:ss word\r\nbob:{hash}\n  # indented comment\n"),
        )
        .unwrap();
        let a = Auth::load(Some(&f), Some("carol"), Some("c"))
            .unwrap()
            .unwrap();
        assert_eq!(a.user_count(), 3);
        assert!(a.credentials_valid(&basic("alice", "pa:ss word")));
        assert!(a.credentials_valid(&basic("bob", "s3cret")));
        assert!(a.credentials_valid(&basic("carol", "c")));
        assert!(a.credentials_valid(b"basic Y2Fyb2w6Yw=="));
        assert!(!a.credentials_valid(&basic("bob", &hash)));
        assert!(!a.credentials_valid(&basic("alice", "pa:ss")));
        assert!(!a.credentials_valid(&basic("carol", "pa:ss word")));
        assert!(!a.credentials_valid(&basic("", "")));
        assert!(!a.credentials_valid(b"Bearer x"));
        assert!(!a.credentials_valid(b"Basic !!!"));
        assert!(!a.credentials_valid(b""));

        assert!(Auth::load(None, None, None).unwrap().is_none());
        assert!(Auth::load(None, Some("u"), None).is_err());
        assert!(Auth::load(Some(&dir.path().join("missing")), None, None).is_err());
        std::fs::write(&f, "# only a comment\n\n").unwrap();
        assert!(Auth::load(Some(&f), None, None).is_err());
        std::fs::write(&f, "alice\n").unwrap();
        let e = Auth::load(Some(&f), None, None).err().unwrap().to_string();
        assert!(e.contains("line 1") && !e.contains("alice"), "{e}");
        std::fs::write(&f, "alice:\n").unwrap();
        assert!(Auth::load(Some(&f), None, None).is_err());
    }

    #[test]
    fn lockout_after_repeated_failures() {
        let a = Auth::load(None, Some("u"), Some("p")).unwrap().unwrap();
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        let other: IpAddr = "192.0.2.2".parse().unwrap();
        let good = basic("u", "p");
        let bad = basic("u", "x");
        // no credentials never counts
        for _ in 0..20 {
            assert_eq!(a.check(Some(ip), None), Verdict::Denied);
        }
        for _ in 0..MAX_FAILURES - 1 {
            assert_eq!(a.check(Some(ip), Some(&bad)), Verdict::Denied);
        }
        // a success resets the counter
        assert_eq!(a.check(Some(ip), Some(&good)), Verdict::Ok);
        for _ in 0..MAX_FAILURES {
            assert_eq!(a.check(Some(ip), Some(&bad)), Verdict::Denied);
        }
        assert!(matches!(a.check(Some(ip), Some(&good)), Verdict::LockedOut(s) if s > 800));
        assert_eq!(a.check(Some(other), Some(&good)), Verdict::Ok);
        // expired lockouts are lifted
        a.failures
            .lock()
            .unwrap()
            .get_mut(&ip)
            .unwrap()
            .locked_until = Some(Instant::now() - Duration::from_secs(1));
        assert_eq!(a.check(Some(ip), Some(&good)), Verdict::Ok);
    }
}
