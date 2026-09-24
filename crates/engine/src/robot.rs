//! Robots: state, intents, pending blocking operations, and per-robot stats.

use crate::config::Config;
use crate::event::Event;
use robolang::Vm;
use std::collections::VecDeque;

/// A blocking operation in progress (created from a `BlockRequest`).
/// The engine advances it every tick; when it completes, the robot's VM
/// resumes with `Value::Null`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pending {
    /// Move `remaining` units forward (`dir = 1`) or backward (`dir = -1`).
    Move {
        remaining: f64,
        dir: f64,
    },
    TurnBody {
        remaining: f64,
    },
    TurnGun {
        remaining: f64,
    },
    TurnRadar {
        remaining: f64,
    },
    AwaitTick {
        n: u32,
    },
}

#[derive(Clone, Debug, Default)]
pub struct RobotStats {
    pub fired: u32,
    pub bullet_hits: u32,
    pub damage_dealt: f64,
    pub damage_taken: f64,
    pub budget_strikes: u32,
    /// log() lines recorded so far (capped at `Config::max_logs`).
    pub logs: usize,
}

pub struct Robot {
    pub id: usize,
    pub name: String,
    // Pose. Angles in degrees; 0 = north (up), increasing clockwise.
    /// Heading of the tank body.
    pub body_heading: f64,
    /// Gun angle relative to the body.
    pub gun_rel: f64,
    /// Radar angle relative to the gun.
    pub radar_rel: f64,
    /// Radar heading at the start of the current tick (for swept-beam
    /// detection: a target anywhere in the arc the beam passed through this
    /// tick is detected, so fast sweeps cannot step over targets).
    pub prev_radar_heading: f64,
    pub x: f64,
    pub y: f64,
    /// Signed velocity along the body heading.
    pub velocity: f64,
    pub energy: f64,
    /// Gun heat in ticks; fires only when <= 0.
    pub gun_heat: f64,
    pub alive: bool,
    /// Why the robot died (fault text), if it forfeited.
    pub fault: Option<String>,
    // Intentions (persist until changed by robot code or a pending op).
    pub intent_velocity: f64,
    pub intent_body_rate: f64,
    pub intent_gun_rate: f64,
    pub intent_radar_rate: f64,
    pub pending: Option<Pending>,
    pub vm: Option<Vm>,
    pub events: VecDeque<Event>,
    /// Event most recently returned by pop_event(); accessors read it.
    pub current_event: Option<Event>,
    pub stats: RobotStats,
}

impl Robot {
    pub fn gun_heading(&self) -> f64 {
        norm_deg(self.body_heading + self.gun_rel)
    }

    pub fn radar_heading(&self) -> f64 {
        norm_deg(self.body_heading + self.gun_rel + self.radar_rel)
    }

    pub fn queue_event(&mut self, cfg: &Config, e: Event) {
        if self.events.len() < cfg.event_queue_cap {
            self.events.push_back(e);
        }
    }

    /// Max body turn rate at the current speed.
    pub fn body_rate_limit(&self, cfg: &Config) -> f64 {
        (cfg.max_body_rate - cfg.body_rate_speed_factor * self.velocity.abs()).max(0.0)
    }
}

/// Normalize an angle to [0, 360).
pub fn norm_deg(a: f64) -> f64 {
    let mut a = a % 360.0;
    if a < 0.0 {
        a += 360.0;
    }
    if a >= 360.0 {
        a -= 360.0;
    }
    a
}

/// Shortest signed difference a - b, in (-180, 180].
pub fn ang_diff(a: f64, b: f64) -> f64 {
    let mut d = (a - b) % 360.0;
    if d > 180.0 {
        d -= 360.0;
    } else if d <= -180.0 {
        d += 360.0;
    }
    d
}

/// Absolute bearing (deg, 0 = north, clockwise) from (x0, y0) to (x1, y1).
pub fn bearing_deg(x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
    let dx = x1 - x0;
    let dy = y1 - y0;
    if dx == 0.0 && dy == 0.0 {
        return 0.0;
    }
    norm_deg((dx).atan2(-dy).to_degrees())
}

pub fn dist(x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
    ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt()
}

/// Distance from point p to segment (ax, ay)-(bx, by).
pub fn point_segment_dist(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    if len2 == 0.0 {
        return dist(px, py, ax, ay);
    }
    let t = ((px - ax) * dx + (py - ay) * dy) / len2;
    let t = t.clamp(0.0, 1.0);
    dist(px, py, ax + t * dx, ay + t * dy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angle_helpers() {
        assert!((ang_diff(350.0, 10.0) + 20.0).abs() < 1e-9);
        assert!((ang_diff(10.0, 350.0) - 20.0).abs() < 1e-9);
        assert!((norm_deg(-10.0) - 350.0).abs() < 1e-9);
        assert!((norm_deg(370.0) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn bearing_convention() {
        // 0 = north (up), 90 = east (right), 180 = south, 270 = west.
        assert!((bearing_deg(0.0, 0.0, 0.0, -10.0)).abs() < 1e-9);
        assert!((bearing_deg(0.0, 0.0, 10.0, 0.0) - 90.0).abs() < 1e-9);
        assert!((bearing_deg(0.0, 0.0, 0.0, 10.0) - 180.0).abs() < 1e-9);
        assert!((bearing_deg(0.0, 0.0, -10.0, 0.0) - 270.0).abs() < 1e-9);
    }

    #[test]
    fn segment_distance() {
        assert!((point_segment_dist(5.0, 5.0, 0.0, 0.0, 10.0, 0.0) - 5.0).abs() < 1e-9);
        assert!((point_segment_dist(-1.0, 0.0, 0.0, 0.0, 10.0, 0.0) - 1.0).abs() < 1e-9);
        assert!((point_segment_dist(20.0, 0.0, 0.0, 0.0, 10.0, 0.0) - 10.0).abs() < 1e-9);
    }
}
