//! Replay snapshots and the JSON replay format consumed by the static
//! web viewer (`viewer/index.html`).
//!
//! Format (version 1):
//! ```json
//! {
//!   "format": "tank-replay", "version": 1,
//!   "seed": 42, "arena": {"w": 1000, "h": 700},
//!   "robots": [{"name": "alpha", "color": "#e74c3c"}],
//!   "result": {"winner": 0, "reason": "last_standing", "tick": 321},
//!   "ticks": [
//!     {"t": 0, "r": [[x,y,body,gun,radar,energy,alive],...],
//!      "b": [[x,y,owner,power],...], "e": ["..."]}
//!   ]
//! }
//! ```

use crate::battle::{Battle, EndReason};

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub tick: u32,
    /// Per robot: x, y, body heading, gun heading (absolute), radar heading
    /// (absolute), energy, alive (0/1).
    pub robots: Vec<[f64; 7]>,
    /// Per live bullet: x, y, owner id, power.
    pub bullets: Vec<[f64; 4]>,
    /// Human-readable events for this tick.
    pub events: Vec<String>,
}

fn num(v: f64) -> String {
    // Two decimals, trailing zeros trimmed. Deterministic output.
    let s = format!("{:.2}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" || s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

pub const ROBOT_COLORS: &[&str] = &[
    "#e74c3c", // red
    "#3498db", // blue
    "#2ecc71", // green
    "#f39c12", // orange
    "#9b59b6", // purple
    "#1abc9c", // teal
    "#e91e63", // pink
    "#cddc39", // lime
];

/// Serialize a finished battle to replay JSON. Byte-identical for identical
/// battles (same robots, same seed, same engine build).
pub fn write_replay(battle: &Battle) -> String {
    let mut out = String::with_capacity(64 * 1024);
    out.push_str("{\"format\":\"tank-replay\",\"version\":1,");
    out.push_str(&format!("\"seed\":{},", battle.seed));
    out.push_str(&format!(
        "\"arena\":{{\"w\":{},\"h\":{},\"beam\":{}}},",
        num(battle.cfg.arena_w),
        num(battle.cfg.arena_h),
        num(battle.cfg.radar_beam)
    ));
    out.push_str("\"robots\":[");
    for (i, r) in battle.robots.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let color = ROBOT_COLORS[i % ROBOT_COLORS.len()];
        out.push_str(&format!(
            "{{\"name\":\"{}\",\"color\":\"{}\"}}",
            json_escape(&r.name),
            color
        ));
    }
    out.push_str("],");
    if let Some(res) = &battle.result {
        let reason = match res.reason {
            EndReason::LastStanding => "last_standing",
            EndReason::TickLimit => "tick_limit",
        };
        out.push_str(&format!(
            "\"result\":{{\"winner\":{},\"reason\":\"{}\",\"tick\":{}}},",
            match res.winner {
                Some(w) => w.to_string(),
                None => "null".to_string(),
            },
            reason,
            res.tick
        ));
    }
    out.push_str("\"ticks\":[");
    for (i, s) in battle.snapshots.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!("{{\"t\":{},\"r\":[", s.tick));
        for (j, r) in s.robots.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "[{},{},{},{},{},{},{}]",
                num(r[0]),
                num(r[1]),
                num(r[2]),
                num(r[3]),
                num(r[4]),
                num(r[5]),
                if r[6] > 0.5 { "1" } else { "0" }
            ));
        }
        out.push_str("],\"b\":[");
        for (j, b) in s.bullets.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "[{},{},{},{}]",
                num(b[0]),
                num(b[1]),
                b[2] as i64,
                num(b[3])
            ));
        }
        out.push_str("],\"e\":[");
        for (j, e) in s.events.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            out.push_str(&format!("\"{}\"", json_escape(e)));
        }
        out.push_str("]}");
    }
    out.push_str("]}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn num_formatting() {
        assert_eq!(num(500.0), "500");
        assert_eq!(num(123.456), "123.46");
        assert_eq!(num(-0.0), "0");
        assert_eq!(num(0.5), "0.5");
    }

    #[test]
    fn escaping() {
        assert_eq!(json_escape("a\"b\\c\n"), "a\\\"b\\\\c\\n");
    }
}
