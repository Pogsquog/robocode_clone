//! Simulation constants. Single source of truth for game balance:
//! every tuning knob lives here.

/// All tunable battle constants.
#[derive(Clone, Debug)]
pub struct Config {
    // Arena
    pub arena_w: f64,
    pub arena_h: f64,

    // Tank
    pub tank_radius: f64,
    pub start_energy: f64,
    pub max_speed: f64,
    pub accel: f64,
    pub decel: f64,
    /// Max body turn rate (deg/tick) at zero speed.
    pub max_body_rate: f64,
    /// Body turn rate loss per unit of speed (deg/tick per unit).
    pub body_rate_speed_factor: f64,
    pub max_gun_rate: f64,
    pub max_radar_rate: f64,

    // Radar
    /// Full width of the radar beam in degrees ("narrow active radar").
    pub radar_beam: f64,

    // Gun & bullets
    pub min_fire_power: f64,
    pub max_fire_power: f64,
    /// Bullet damage = damage_factor * power.
    pub bullet_damage_factor: f64,
    /// Bullet speed = speed_base - speed_factor * power.
    pub bullet_speed_base: f64,
    pub bullet_speed_factor: f64,
    /// Energy returned to the shooter when its bullet hits.
    pub bullet_energy_return: f64,
    /// Gun cooldown (ticks) = cooldown_base + cooldown_factor * power.
    pub cooldown_base: f64,
    pub cooldown_factor: f64,
    /// Effective hit radius of a bullet against a tank = tank_radius + this.
    pub bullet_pad: f64,

    // Damage
    pub wall_damage: f64,
    pub collision_damage: f64,

    // Sandbox / fairness
    /// VM instructions allowed per robot per tick.
    pub instruction_budget: u32,
    /// Forfeit threshold for repeatedly exceeding the instruction budget.
    pub max_budget_strikes: u32,
    /// Max events waiting in a robot's event queue.
    pub event_queue_cap: usize,
    /// Max total log() lines per robot per battle.
    pub max_logs: usize,

    // Misc
    /// Angular/positional epsilon.
    pub epsilon: f64,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            arena_w: 1000.0,
            arena_h: 700.0,
            tank_radius: 18.0,
            start_energy: 100.0,
            max_speed: 8.0,
            accel: 2.0,
            decel: 4.0,
            max_body_rate: 10.0,
            body_rate_speed_factor: 0.75,
            max_gun_rate: 20.0,
            max_radar_rate: 45.0,
            radar_beam: 18.0,
            min_fire_power: 0.1,
            max_fire_power: 3.0,
            bullet_damage_factor: 4.0,
            bullet_speed_base: 20.0,
            bullet_speed_factor: 4.0,
            bullet_energy_return: 3.0,
            cooldown_base: 5.0,
            cooldown_factor: 5.0,
            bullet_pad: 3.0,
            wall_damage: 0.5,
            collision_damage: 0.5,
            instruction_budget: 5000,
            max_budget_strikes: 30,
            event_queue_cap: 64,
            max_logs: 500,
            epsilon: 1e-9,
        }
    }
}
