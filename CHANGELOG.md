# Changelog

## v0.1.4

New, all optional and off by default – existing setups behave as before:

- **Basic auth**: `--auth-file` / `STREAGIT_AUTH_FILE` (`user:password`
  lines, optionally `sha256:<hex>` passwords) and `STREAGIT_AUTH_USER` +
  `STREAGIT_AUTH_PASSWORD`. Guards every request except `<prefix>/healthz`,
  constant-time comparison, per-IP lockout after 10 failed logins (15 min),
  `Cache-Control: private`.
- **Index page**: `--index true` serves a stagit-index compatible overview
  of all repositories below `--root` at `<prefix>/` (and per directory),
  newest first; `--index-depth` (3), `--index-ttl` (60s).
- **Bare repositories without `.git` suffix** are served with
  `--allow-bare-dirs true` (default: on with `--index`).
- **`--exclude PATTERN`** (repeatable, `STREAGIT_EXCLUDE` comma-separated)
  hides paths below the root from the index and from direct URLs.
- **`--name-from path`**: name repositories by their path below the root.

Fixed:

- Repositories owned by another user than the streagit process (reduced
  trust in gix) could not show blobs larger than 16 MiB.

Documented and tested: streagit never writes to the repositories and works
on a read-only mount owned by another user.
