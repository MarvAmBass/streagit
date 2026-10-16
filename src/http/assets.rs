//! style.css / logo.png / favicon.png (SPEC §4.13, §5.3): stagit 1.3's files (MIT,
//! (c) Hiltjo Posthuma) embedded, optionally overridden from `--assets-dir`.

use std::path::Path;

use bytes::Bytes;

pub struct Asset {
    pub name: &'static str,
    pub content_type: &'static str,
    pub body: Bytes,
    pub etag: String,
}

pub struct Assets {
    pub list: Vec<Asset>,
}

const EMBEDDED: [(&str, &str, &[u8]); 3] = [
    (
        "style.css",
        "text/css; charset=utf-8",
        include_bytes!("../../assets/style.css"),
    ),
    (
        "logo.png",
        "image/png",
        include_bytes!("../../assets/logo.png"),
    ),
    (
        "favicon.png",
        "image/png",
        include_bytes!("../../assets/favicon.png"),
    ),
];

impl Assets {
    pub fn load(dir: Option<&Path>) -> Assets {
        let mut list = Vec::new();
        for (name, ct, embedded) in EMBEDDED {
            let body = match dir.map(|d| std::fs::read(d.join(name))) {
                Some(Ok(data)) => {
                    tracing::info!(asset = name, "using asset from --assets-dir");
                    Bytes::from(data)
                }
                Some(Err(e)) => {
                    tracing::warn!(asset = name, error = %e, "asset not readable in --assets-dir, using embedded");
                    Bytes::from_static(embedded)
                }
                None => Bytes::from_static(embedded),
            };
            let etag = format!("\"{:016x}\"", xxhash_rust::xxh3::xxh3_64(&body));
            list.push(Asset {
                name,
                content_type: ct,
                body,
                etag,
            });
        }
        Assets { list }
    }

    pub fn get(&self, name: &[u8]) -> Option<&Asset> {
        self.list.iter().find(|a| a.name.as_bytes() == name)
    }
}
