//! Command line / environment configuration (SPEC §11).

use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, ValueEnum};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum NameFrom {
    /// The slot's last path component (gitbackup's repository name).
    Slot,
    /// The git directory name with `.git` stripped (what stagit does).
    Dir,
    /// `<owner>/<slot>`; owner from the `owner` file or the directory two levels up.
    OwnerSlot,
    /// The git directory's path below the root with `.git` stripped (`team/tool`).
    Path,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Robots {
    Allow,
    Disallow,
}

fn parse_bytes(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let (num, mult) = if let Some(n) = s.strip_suffix("GiB").or_else(|| s.strip_suffix('G')) {
        (n, 1u64 << 30)
    } else if let Some(n) = s.strip_suffix("MiB").or_else(|| s.strip_suffix('M')) {
        (n, 1u64 << 20)
    } else if let Some(n) = s.strip_suffix("KiB").or_else(|| s.strip_suffix('K')) {
        (n, 1u64 << 10)
    } else {
        (s, 1)
    };
    num.trim()
        .parse::<u64>()
        .map(|n| n * mult)
        .map_err(|e| format!("invalid byte size {s:?}: {e}"))
}

fn parse_duration(s: &str) -> Result<Duration, String> {
    humantime::parse_duration(s).map_err(|e| e.to_string())
}

/// All runtime options. Every flag can also be given as `STREAGIT_<NAME>`.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "streagit",
    version,
    about = "On-demand, streaming, stagit-compatible git web renderer"
)]
pub struct Config {
    /// Bind address.
    #[arg(long, env = "STREAGIT_LISTEN", default_value = "127.0.0.1:8081")]
    pub listen: String,

    /// Data root containing the repository slots.
    #[arg(long, env = "STREAGIT_ROOT")]
    pub root: PathBuf,

    /// URL prefix stripped before resolution (`/browse` for gitbackup).
    #[arg(long, env = "STREAGIT_PREFIX", default_value = "/")]
    pub prefix: String,

    /// Absolute URL of `--prefix` as seen by users; used for atom feed links.
    #[arg(long, env = "STREAGIT_PUBLIC_URL")]
    pub public_url: Option<String>,

    /// Log rows per page (max 500).
    #[arg(long, env = "STREAGIT_PAGE_SIZE", default_value_t = 100)]
    pub page_size: usize,

    /// Concurrent renders (default: cores/4, min 1).
    #[arg(long, env = "STREAGIT_RENDER_THREADS")]
    pub render_threads: Option<usize>,

    /// Maximum number of requests waiting for a render permit.
    #[arg(long, env = "STREAGIT_MAX_QUEUE", default_value_t = 32)]
    pub max_queue: usize,

    /// How long a request waits for a render permit before 503.
    #[arg(long, env = "STREAGIT_QUEUE_TIMEOUT", default_value = "10s", value_parser = parse_duration)]
    pub queue_timeout: Duration,

    /// Time limit for one render (log pages get twice this).
    #[arg(long, env = "STREAGIT_RENDER_TIMEOUT", default_value = "30s", value_parser = parse_duration)]
    pub render_timeout: Duration,

    /// Render cache size in bytes.
    #[arg(long, env = "STREAGIT_CACHE_BYTES", default_value = "64MiB", value_parser = parse_bytes)]
    pub cache_bytes: u64,

    /// Largest page stored in the render cache.
    #[arg(long, env = "STREAGIT_CACHE_MAX_ENTRY", default_value = "1MiB", value_parser = parse_bytes)]
    pub cache_max_entry: u64,

    /// Diffstat cache entries.
    #[arg(long, env = "STREAGIT_STATS_ENTRIES", default_value_t = 2_000_000)]
    pub stats_entries: usize,

    /// Directory for the persistent diffstat cache (optional).
    #[arg(long, env = "STREAGIT_CACHE_DIR")]
    pub cache_dir: Option<PathBuf>,

    /// Number of repository handles kept open.
    #[arg(long, env = "STREAGIT_OPEN_REPOS", default_value_t = 64)]
    pub open_repos: usize,

    /// gix object cache per render.
    #[arg(long, env = "STREAGIT_OBJECT_CACHE_BYTES", default_value = "8MiB", value_parser = parse_bytes)]
    pub object_cache_bytes: u64,

    /// Blob pages are truncated beyond this size.
    #[arg(long, env = "STREAGIT_MAX_BLOB_BYTES", default_value = "2MiB", value_parser = parse_bytes)]
    pub max_blob_bytes: u64,

    /// Deltas with a side larger than this are not diffed.
    #[arg(long, env = "STREAGIT_MAX_DIFF_BLOB_BYTES", default_value = "4MiB", value_parser = parse_bytes)]
    pub max_diff_blob_bytes: u64,

    /// Total blob bytes loaded for one commit diff.
    #[arg(long, env = "STREAGIT_MAX_COMMIT_DIFF_BYTES", default_value = "64MiB", value_parser = parse_bytes)]
    pub max_commit_diff_bytes: u64,

    /// Maximum rows on files.html.
    #[arg(long, env = "STREAGIT_MAX_FILES", default_value_t = 20000)]
    pub max_files: usize,

    /// Show line counts (`NL`) on files.html like stagit (reads every blob).
    #[arg(long, env = "STREAGIT_FILES_LINE_COUNTS", default_value_t = false)]
    pub files_line_counts: bool,

    /// Entries in atom.xml / tags.xml.
    #[arg(long, env = "STREAGIT_FEED_ENTRIES", default_value_t = 100)]
    pub feed_entries: usize,

    /// Where the repository name shown on pages comes from.
    #[arg(long, env = "STREAGIT_NAME_FROM", value_enum, default_value_t = NameFrom::Slot)]
    pub name_from: NameFrom,

    /// Directory with style.css / logo.png / favicon.png overriding the embedded ones.
    #[arg(long, env = "STREAGIT_ASSETS_DIR")]
    pub assets_dir: Option<PathBuf>,

    /// Allow direct `<name>.git/...` URLs.
    #[arg(long, env = "STREAGIT_ALLOW_GIT_PATHS", default_value_t = true, action = clap::ArgAction::Set)]
    pub allow_git_paths: bool,

    /// Link ref names on refs.html to `log.html?ref=`.
    #[arg(long, env = "STREAGIT_LINK_REFS", default_value_t = true, action = clap::ArgAction::Set)]
    pub link_refs: bool,

    /// robots.txt policy.
    #[arg(long, env = "STREAGIT_ROBOTS", value_enum, default_value_t = Robots::Disallow)]
    pub robots: Robots,

    /// Process niceness applied at startup.
    #[arg(long, env = "STREAGIT_NICE", default_value_t = 0)]
    pub nice: i32,

    /// gzip responses.
    #[arg(long, env = "STREAGIT_COMPRESSION", default_value_t = true, action = clap::ArgAction::Set)]
    pub compression: bool,

    /// Maximum concurrent connections.
    #[arg(long, env = "STREAGIT_MAX_CONNECTIONS", default_value_t = 256)]
    pub max_connections: usize,

    /// Log level / filter (RUST_LOG syntax).
    #[arg(long, env = "STREAGIT_LOG_LEVEL", default_value = "info")]
    pub log_level: String,

    /// File with `user:password` lines; enables HTTP basic auth (see README, Security).
    #[arg(long, env = "STREAGIT_AUTH_FILE")]
    pub auth_file: Option<PathBuf>,

    /// From `STREAGIT_AUTH_USER` only (never a command line flag), see `Config::load_env`.
    #[arg(skip)]
    pub auth_user: Option<String>,

    /// From `STREAGIT_AUTH_PASSWORD` only (never a command line flag).
    #[arg(skip)]
    pub auth_password: Option<Secret>,

    /// Serve an overview of all repositories below the root at `<prefix>/`.
    #[arg(long, env = "STREAGIT_INDEX", default_value_t = false, action = clap::ArgAction::Set)]
    pub index: bool,

    /// Directory levels below the root searched for repositories by the index.
    #[arg(long, env = "STREAGIT_INDEX_DEPTH", default_value_t = 3)]
    pub index_depth: usize,

    /// How long the repository list of the index is reused before the root is walked again.
    #[arg(long, env = "STREAGIT_INDEX_TTL", default_value = "60s", value_parser = parse_duration)]
    pub index_ttl: Duration,

    /// Serve bare repositories whose directory name does not end in `.git`
    /// (default: the value of `--index`).
    #[arg(long, env = "STREAGIT_ALLOW_BARE_DIRS", action = clap::ArgAction::Set)]
    pub allow_bare_dirs: Option<bool>,

    /// Path or glob below the root that is neither listed nor served (repeatable;
    /// `STREAGIT_EXCLUDE` takes a comma-separated list).
    #[arg(long, env = "STREAGIT_EXCLUDE", value_delimiter = ',')]
    pub exclude: Vec<String>,
}

/// A string that is never printed (`Debug` shows `<redacted>`).
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

impl Config {
    /// Read the settings that only come from the environment
    /// (`STREAGIT_AUTH_USER`, `STREAGIT_AUTH_PASSWORD`).
    pub fn load_env(&mut self) {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        self.auth_user = var("STREAGIT_AUTH_USER");
        self.auth_password = var("STREAGIT_AUTH_PASSWORD").map(Secret);
    }

    /// Whether suffix-less bare repository directories are served.
    pub fn allow_bare_dirs(&self) -> bool {
        self.allow_bare_dirs.unwrap_or(self.index)
    }

    /// Effective number of render permits.
    pub fn render_threads(&self) -> usize {
        self.render_threads.unwrap_or_else(|| {
            let cores = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1);
            (cores / 4).max(1)
        })
    }

    pub fn page_size(&self) -> usize {
        self.page_size.clamp(1, 500)
    }

    /// The URL prefix normalised to start with `/` and not end with one (`""` for `/`).
    pub fn prefix(&self) -> String {
        let p = self.prefix.trim().trim_end_matches('/');
        if p.is_empty() {
            String::new()
        } else if p.starts_with('/') {
            p.to_string()
        } else {
            format!("/{p}")
        }
    }

    /// Template version: changes whenever the output of a page changes for the same ids.
    pub fn template_version(&self) -> String {
        let s = format!(
            "{}|{:?}|{}|{}|{}",
            env!("CARGO_PKG_VERSION"),
            self.name_from,
            self.files_line_counts,
            self.link_refs,
            self.page_size()
        );
        format!("{:08x}", xxhash_rust::xxh3::xxh3_64(s.as_bytes()) as u32)
    }

    /// A configuration with defaults for tests and embedding.
    pub fn for_root(root: PathBuf) -> Self {
        Config::parse_from(["streagit", "--root", root.to_str().expect("utf-8 root")])
    }
}
