//! Events delivered to robots. v1 uses polling (`pop_event()` + accessors)
//! rather than interrupt handlers.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// Radar locked onto an enemy this tick.
    Scanned,
    /// One of my bullets hit an enemy.
    BulletHit,
    /// I was hit by an enemy bullet.
    HitByBullet,
    /// One of my bullets left the arena without hitting.
    BulletMissed,
    /// I hit a wall.
    Wall,
    /// I collided with another robot.
    RobotCollision,
    /// Another robot died.
    RobotDeath,
}

#[derive(Clone, Debug)]
pub struct Event {
    pub kind: EventKind,
    /// Position (scan target, or collision/bullet source).
    pub x: f64,
    pub y: f64,
    /// Scanned robot's heading (deg, 0 = north, clockwise).
    pub heading: f64,
    /// Scanned robot's velocity.
    pub velocity: f64,
    /// Scanned robot's energy.
    pub energy: f64,
    /// Absolute bearing (deg) from me to whatever the event is about.
    pub bearing: f64,
    /// Distance from me to whatever the event is about.
    pub dist: f64,
    /// Bullet power (BulletHit / HitByBullet).
    pub power: f64,
    /// Name of the other robot involved.
    pub name: String,
}

impl Event {
    pub fn empty(kind: EventKind) -> Event {
        Event {
            kind,
            x: 0.0,
            y: 0.0,
            heading: 0.0,
            velocity: 0.0,
            energy: 0.0,
            bearing: 0.0,
            dist: 0.0,
            power: 0.0,
            name: String::new(),
        }
    }
}

impl EventKind {
    /// The string `pop_event()` returns for this kind.
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Scanned => "scanned",
            EventKind::BulletHit => "bullet_hit",
            EventKind::HitByBullet => "hit_by_bullet",
            EventKind::BulletMissed => "bullet_missed",
            EventKind::Wall => "wall",
            EventKind::RobotCollision => "robot_collision",
            EventKind::RobotDeath => "robot_death",
        }
    }
}
