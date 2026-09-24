//! tank: command-line interface for the tank battle game.
//!
//! Usage:
//!   tank run <bot.bot> <bot2.bot> [more...] [options]
//!   tank check <bot.bot> [more...]
//!
//! Options for `run`:
//!   --seed N        battle seed (default: derived from the clock)
//!   --max-ticks N   tick limit (default 3000)
//!   --out FILE      replay output path (default tank_replay.json)
//!   --verbose       print robot log lines as the battle runs
//!   --quiet         suppress the stats table

use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        print_usage();
        return ExitCode::from(2);
    }
    match args[0].as_str() {
        "run" => cmd_run(&args[1..]),
        "check" => cmd_check(&args[1..]),
        "help" | "--help" | "-h" => {
            print_usage();
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("unknown command '{}'", other);
            print_usage();
            ExitCode::from(2)
        }
    }
}

fn print_usage() {
    println!(
        "tank - robocode-style tank battles with sandboxed robot AI\n\
         \n\
         USAGE:\n\
         \x20 tank run <bot.bot> <bot2.bot> [more...] [options]\n\
         \x20      Fights the given robots and writes a replay file.\n\
         \x20 tank check <bot.bot> [more...]\n\
         \x20      Validates robot source without running a battle.\n\
         \n\
         OPTIONS (run):\n\
         \x20 --seed N        battle seed (default: from the clock)\n\
         \x20 --max-ticks N   tick limit (default 3000)\n\
         \x20 --out FILE      replay path (default tank_replay.json)\n\
         \x20 --verbose       show robot log output\n\
         \x20 --quiet         hide the stats table\n\
         \n\
         Watch replays by opening viewer/index.html in a browser and loading\n\
         the replay file."
    );
}

fn cmd_check(args: &[String]) -> ExitCode {
    let paths: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if paths.is_empty() {
        eprintln!("check: no robot files given");
        return ExitCode::from(2);
    }
    let mut failed = false;
    for path in paths {
        match std::fs::read_to_string(path) {
            Ok(source) => match robolang::compile(&source) {
                Ok(_) => println!("OK: {}", path),
                Err(e) => {
                    println!("FAIL: {} (line {}): {}", path, e.line, e.msg);
                    failed = true;
                }
            },
            Err(e) => {
                println!("FAIL: {}: {}", path, e);
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

struct RunOptions {
    seed: Option<u64>,
    max_ticks: u32,
    out: String,
    verbose: bool,
    quiet: bool,
}

fn cmd_run(args: &[String]) -> ExitCode {
    let mut paths: Vec<String> = Vec::new();
    let mut opts = RunOptions {
        seed: None,
        max_ticks: 3000,
        out: "tank_replay.json".into(),
        verbose: false,
        quiet: false,
    };
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let next = || args.get(i + 1).cloned();
        match a.as_str() {
            "--seed" => match next() {
                Some(v) => {
                    opts.seed = v.parse().ok();
                    if opts.seed.is_none() {
                        eprintln!("--seed expects a number, got '{}'", v);
                        return ExitCode::from(2);
                    }
                    i += 2;
                }
                None => {
                    eprintln!("--seed expects a number");
                    return ExitCode::from(2);
                }
            },
            "--max-ticks" => match next() {
                Some(v) => match v.parse() {
                    Ok(n) => {
                        opts.max_ticks = n;
                        i += 2;
                    }
                    Err(_) => {
                        eprintln!("--max-ticks expects a number, got '{}'", v);
                        return ExitCode::from(2);
                    }
                },
                None => {
                    eprintln!("--max-ticks expects a number");
                    return ExitCode::from(2);
                }
            },
            "--out" => match next() {
                Some(v) => {
                    opts.out = v;
                    i += 2;
                }
                None => {
                    eprintln!("--out expects a path");
                    return ExitCode::from(2);
                }
            },
            "--verbose" => {
                opts.verbose = true;
                i += 1;
            }
            "--quiet" => {
                opts.quiet = true;
                i += 1;
            }
            other if other.starts_with("--") => {
                eprintln!("unknown option '{}'", other);
                return ExitCode::from(2);
            }
            other => {
                paths.push(other.to_string());
                i += 1;
            }
        }
    }
    if paths.len() < 2 {
        eprintln!("run: need at least two robot files");
        return ExitCode::from(2);
    }

    // Load and pre-validate all robots (nice errors before the battle starts).
    let mut specs = Vec::new();
    for path in &paths {
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("cannot read '{}': {}", path, e);
                return ExitCode::from(2);
            }
        };
        if let Err(e) = robolang::compile(&source) {
            eprintln!("{} does not compile (line {}): {}", path, e.line, e.msg);
            return ExitCode::from(2);
        }
        // Robot name comes from the file name.
        let name = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        specs.push(engine::RobotSpec { name, source });
    }

    let seed = opts.seed.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1)
    });

    println!(
        "battle: {} | seed {} | max {} ticks",
        specs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(" vs "),
        seed,
        opts.max_ticks
    );

    let mut battle = match engine::Battle::new(engine::Config::default(), &specs, seed, opts.max_ticks)
    {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{}", e);
            return ExitCode::FAILURE;
        }
    };
    battle.run();

    // Result.
    let res = battle.result.as_ref().expect("battle finished");
    let reason = match res.reason {
        engine::EndReason::LastStanding => "last robot standing",
        engine::EndReason::TickLimit => "tick limit (highest energy wins)",
    };
    match res.winner {
        Some(w) => println!(
            "winner: {} at tick {} ({})",
            battle.robots[w].name, res.tick, reason
        ),
        None => println!("draw at tick {} ({})", res.tick, reason),
    }

    // Stats table.
    if !opts.quiet {
        println!(
            "\n{:<12} {:>7} {:>7} {:>7} {:>7} {:>9} {:>9}  notes",
            "robot", "energy", "fired", "hits", "acc%", "dealt", "taken"
        );
        for r in &battle.robots {
            let acc = if r.stats.fired > 0 {
                100.0 * r.stats.bullet_hits as f64 / r.stats.fired as f64
            } else {
                0.0
            };
            let notes = if let Some(f) = &r.fault {
                format!("forfeit: {}", f)
            } else if !r.alive {
                "destroyed".to_string()
            } else {
                "".to_string()
            };
            println!(
                "{:<12} {:>7.1} {:>7} {:>7} {:>6.1}% {:>9.1} {:>9.1}  {}",
                r.name,
                r.energy.max(0.0),
                r.stats.fired,
                r.stats.bullet_hits,
                acc,
                r.stats.damage_dealt,
                r.stats.damage_taken,
                notes
            );
        }
    }

    // Robot logs.
    if opts.verbose {
        println!("\nrobot log:");
        for (tick, id, msg) in &battle.logs {
            let name = &battle.robots[*id].name;
            println!("  [{:>4}] {}: {}", tick, name, msg);
        }
    }

    // Replay.
    let replay = engine::write_replay(&battle);
    if let Err(e) = std::fs::write(&opts.out, replay) {
        eprintln!("cannot write replay '{}': {}", opts.out, e);
        return ExitCode::FAILURE;
    }
    println!("\nreplay written to {} (open viewer/index.html to watch)", opts.out);
    ExitCode::SUCCESS
}
