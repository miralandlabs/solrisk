use tracing_log::LogTracer;
use tracing_subscriber::{fmt::format::FmtSpan, prelude::*};

static INIT_TRACING: std::sync::Once = std::sync::Once::new();

pub fn init_tracing() {
    INIT_TRACING.call_once(|| {
        if let Err(e) = LogTracer::init() {
            eprintln!(
                "Failed to initialize LogTracer: {}. Continuing without log bridging.",
                e
            );
        }

        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_target(true)
            .with_level(true)
            .with_span_events(FmtSpan::NONE)
            .compact();

        let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

        if let Err(e) = tracing_subscriber::registry()
            .with(fmt_layer)
            .with(env_filter)
            .try_init()
        {
            eprintln!(
                "Failed to initialize tracing subscriber: {}. Logs may be limited.",
                e
            );
        }
    });
}
