//! Integration tests: drive the `tank` binary as a real process.

use std::fs;
use std::process::Command;

fn tank() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tank"))
}

fn example(name: &str) -> String {
    format!("{}/../../examples/{}", env!("CARGO_MANIFEST_DIR"), name)
}

#[test]
fn check_accepts_examples_and_rejects_garbage() {
    for bot in ["sweeper.bot", "corner.bot", "tracker.bot", "sniper.bot"] {
        let out = tank()
            .args(["check", &example(bot)])
            .output()
            .expect("run tank");
        assert!(out.status.success(), "{} failed check: {:?}", bot, out);
    }
    let bad = std::env::temp_dir().join("tank_bad.bot");
    fs::write(&bad, "func main() { var x = ; }").unwrap();
    let out = tank().arg("check").arg(&bad).output().expect("run tank");
    assert!(!out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("FAIL"), "stdout: {}", stdout);
}

#[test]
fn same_seed_produces_byte_identical_replays_across_processes() {
    let dir = std::env::temp_dir();
    let a = dir.join("tank_det_a.json");
    let b = dir.join("tank_det_b.json");
    for path in [&a, &b] {
        let out = tank()
            .args([
                "run",
                &example("sweeper.bot"),
                &example("corner.bot"),
                "--seed",
                "12345",
                "--quiet",
                "--out",
            ])
            .arg(path)
            .output()
            .expect("run tank");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
    let ra = fs::read(&a).expect("replay a");
    let rb = fs::read(&b).expect("replay b");
    assert_eq!(ra.len(), rb.len());
    assert_eq!(ra, rb, "replays with the same seed must be byte-identical");
}

#[test]
fn different_seeds_produce_different_replays() {
    let dir = std::env::temp_dir();
    let a = dir.join("tank_seed1.json");
    let b = dir.join("tank_seed2.json");
    for (seed, path) in [("1", &a), ("2", &b)] {
        tank()
            .args([
                "run",
                &example("sweeper.bot"),
                &example("tracker.bot"),
                "--seed",
                seed,
                "--quiet",
                "--out",
            ])
            .arg(path)
            .output()
            .expect("run tank");
    }
    assert_ne!(fs::read(&a).unwrap(), fs::read(&b).unwrap());
}

#[test]
fn three_robot_battle_works() {
    let out = std::env::temp_dir().join("tank_three.json");
    let res = tank()
        .args([
            "run",
            &example("sweeper.bot"),
            &example("tracker.bot"),
            &example("corner.bot"),
            "--seed",
            "5",
            "--max-ticks",
            "2000",
            "--quiet",
            "--out",
        ])
        .arg(&out)
        .output()
        .expect("run tank");
    assert!(res.status.success(), "{}", String::from_utf8_lossy(&res.stderr));
    let stdout = String::from_utf8_lossy(&res.stdout);
    assert!(stdout.contains("battle:"), "stdout: {}", stdout);
    let replay = fs::read_to_string(&out).unwrap();
    assert!(replay.contains("\"robots\":["));
    assert!(replay.matches("\"name\":").count() >= 3);
}
