mod capture;
mod cli;
mod compare;
mod config;
mod control;
mod engine;
mod measurement;
mod report;
mod scene;
mod video;

fn main() {
    let command = match cli::parse(std::env::args().skip(1)) {
        Ok(c) => c,
        Err(error) => {
            report::emit("error", serde_json::json!({"message":error}));
            std::process::exit(2);
        }
    };
    match command {
        cli::Command::Help => print!("{}", cli::HELP),
        cli::Command::Version => println!(
            "Ushas Bench {} ({})",
            env!("CARGO_PKG_VERSION"),
            env!("USHAS_BENCH_SOURCE_REVISION")
        ),
        cli::Command::Run(config) => execute(config, None),
        cli::Command::Compare { config, rounds } => execute(config, Some(rounds)),
    }
}

fn execute(mut config: config::RunConfig, rounds: Option<u32>) {
    let result = (|| -> Result<bool, String> {
        config.out = report::reserve_output(&config.out)?;
        control::install();
        let kind = if rounds.is_some() {
            "compare"
        } else {
            config.action.as_str()
        };
        let started = report::utc_now();
        let envelope = report::metadata(&config, kind, &started)?;
        report::emit(
            "started",
            serde_json::json!({"message":format!("Starting {kind}"),"path":config.out,"config":config,"progress":0.0}),
        );
        let value = if let Some(rounds) = rounds {
            compare::run(config.clone(), rounds, envelope)
        } else {
            // Runtime panics retain a failed report. Process signals use the normal
            // asynchronous cancellation path and never unwind across native callbacks.
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                engine::run(config.clone())
            }))
            .unwrap_or_else(|_| report::EngineResult {
                errors: vec!["renderer panicked; inspect the retained diagnostic log".into()],
                ..Default::default()
            });
            report::seal(&config, run, envelope)
        };
        let path = report::write_bundle(&config.out, &value)?;
        report::emit(
            "complete",
            serde_json::json!({"report":path,"valid":value["valid"],"stopped":value["stopped"],"render_fps":value["render_fps"],"progress":1.0}),
        );
        Ok(value["valid"] == true)
    })();
    match result {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(error) => {
            report::emit("error", serde_json::json!({"message":error}));
            std::process::exit(2);
        }
    }
}
