use clap::Parser;
use streagit::config::Config;

fn main() -> anyhow::Result<()> {
    let mut cfg = Config::parse();
    cfg.load_env();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(cfg.log_level.clone()));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
    if cfg.nice != 0 {
        streagit::server::set_nice(cfg.nice);
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(streagit::server::run(cfg))
}
