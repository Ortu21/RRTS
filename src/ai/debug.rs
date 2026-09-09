//! Overlay debug AI (solo grafico): gizmos 3D + pannello punteggio live +
//! controllo velocità demo. Headless non monta il plugin: zero costo di misura.

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use super::{AiConfig, AiMode};

#[derive(Resource, Default)]
pub struct AiDebugState {
    pub last_intents: Vec<String>,
    pub enabled: bool,
}

/// Velocità discrete della demo (Time<Virtual>): i tasti 1-9 restano liberi
/// per futuri control-group RTS, quindi solo +/-/0/Space.
pub const DEMO_SPEEDS: [f32; 6] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0];
pub const DEMO_SPEED_DEFAULT: usize = 2;

#[derive(Resource, Debug)]
pub struct AiSpeed {
    pub index: usize,
    pub paused: bool,
}

impl Default for AiSpeed {
    fn default() -> Self {
        Self {
            index: DEMO_SPEED_DEFAULT,
            paused: false,
        }
    }
}

impl AiSpeed {
    pub fn speed(&self) -> f32 {
        if self.paused {
            0.0
        } else {
            DEMO_SPEEDS[self.index.min(DEMO_SPEEDS.len() - 1)]
        }
    }

    pub fn step_up(&mut self) {
        self.index = (self.index + 1).min(DEMO_SPEEDS.len() - 1);
        self.paused = false;
    }

    pub fn step_down(&mut self) {
        self.index = self.index.saturating_sub(1);
        self.paused = false;
    }

    pub fn reset(&mut self) {
        self.index = DEMO_SPEED_DEFAULT;
        self.paused = false;
    }

    pub fn toggle_pause(&mut self) {
        self.paused = !self.paused;
    }
}

pub struct AiDebugPlugin;
impl Plugin for AiDebugPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AiDebugState>()
            .init_resource::<AiSpeed>()
            .add_systems(Update, ai_speed_keys);
    }
}

/// Tasti velocità demo: +/- step, 0 reset 1x, Space pausa/riprendi.
/// Ascolta sia i KeyCode fisici (layout-indipendenti ma in posizioni diverse
/// per layout: su tastiera italiana '-' è sullo Slash fisico e '+' sul
/// Backslash fisico) sia i caratteri logici ('+', '-', '0'), così funziona su
/// qualsiasi layout. Applicato una sola volta per frame anche se scattano
/// entrambi. Ignorato senza finestra o senza focus (come gli ordini player).
#[allow(clippy::too_many_arguments)]
fn ai_speed_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut typed: MessageReader<KeyboardInput>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    mut speed: ResMut<AiSpeed>,
    mut time: ResMut<Time<Virtual>>,
    config: Res<AiConfig>,
    control: Option<Res<crate::view::SessionControl>>,
    debug: Option<Res<crate::ui::debug::DebugSettings>>,
) {
    if control
        .as_deref()
        .is_some_and(|c| c.player_team().is_some())
        && !debug
            .as_deref()
            .is_some_and(|d| d.on(crate::ui::debug::DebugTool::Simulation))
    {
        return;
    }
    if config.mode == AiMode::Off {
        return;
    }
    if window.is_some_and(|w| !w.focused) {
        return;
    }
    let mut typed_up = false;
    let mut typed_down = false;
    let mut typed_reset = false;
    for event in typed.read() {
        if event.repeat || !event.state.is_pressed() {
            continue;
        }
        if logical_char_pressed(&event.logical_key, &['+', '=']) {
            typed_up = true;
        } else if logical_char_pressed(&event.logical_key, &['-', '_']) {
            typed_down = true;
        } else if logical_char_pressed(&event.logical_key, &['0']) {
            typed_reset = true;
        }
    }
    let mut changed = false;
    if keys.just_pressed(KeyCode::Equal) || keys.just_pressed(KeyCode::NumpadAdd) || typed_up {
        speed.step_up();
        changed = true;
    } else if keys.just_pressed(KeyCode::Minus)
        || keys.just_pressed(KeyCode::NumpadSubtract)
        || typed_down
    {
        speed.step_down();
        changed = true;
    } else if keys.just_pressed(KeyCode::Digit0)
        || keys.just_pressed(KeyCode::Numpad0)
        || keys.just_pressed(KeyCode::Backspace)
        || typed_reset
    {
        speed.reset();
        changed = true;
    } else if keys.just_pressed(KeyCode::Space) {
        speed.toggle_pause();
        changed = true;
    }
    if changed {
        time.set_relative_speed(speed.speed());
    }
}

/// Carattere logico appena digitato (puro e testabile): `Key::Character`
/// segue il layout tastiera, mentre `KeyCode` è la posizione fisica del tasto.
pub fn logical_char_pressed(logical_key: &Key, expected: &[char]) -> bool {
    match logical_key {
        Key::Character(text) => text.chars().any(|c| expected.contains(&c)),
        _ => false,
    }
}

/// Dati punteggio di un team per il pannello live. Struct (non 12 argomenti)
/// così i campi futuri non rompono le firme.
#[cfg(test)]
pub struct TeamScore<'a> {
    pub label: &'a str,
    pub personality: &'a str,
    pub units: usize,
    pub army: usize,
    pub buildings: usize,
    pub sites: usize,
    pub income_metal: f64,
    pub stock_metal: f64,
    pub income_energy: f64,
    pub stock_energy: f64,
    pub power: f32,
    pub orders: u64,
}

/// Riga punteggio per team: unità (di cui armata), edifici (+siti),
/// income/stock metal+energia, potenza Lanchester, ordini emessi.
#[cfg(test)]
pub fn format_team_line(score: &TeamScore) -> String {
    format!(
        "{} {} | {}u ({} army) | {} bldg +{} site | M {:.0}/s [{:.0}] E {:.0}/s [{:.0}] | pow {:.0} | ord {}",
        score.label,
        score.personality,
        score.units,
        score.army,
        score.buildings,
        score.sites,
        score.income_metal,
        score.stock_metal,
        score.income_energy,
        score.stock_energy,
        score.power,
        score.orders,
    )
}

pub fn format_game_clock(elapsed_secs: f32) -> String {
    let total = elapsed_secs.max(0.0) as u64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_steps_clamp_and_pause() {
        let mut speed = AiSpeed::default();
        assert!((speed.speed() - 1.0).abs() < f32::EPSILON);
        speed.step_up();
        assert!((speed.speed() - 2.0).abs() < f32::EPSILON);
        for _ in 0..10 {
            speed.step_up();
        }
        assert!((speed.speed() - 8.0).abs() < f32::EPSILON);
        for _ in 0..10 {
            speed.step_down();
        }
        assert!((speed.speed() - 0.25).abs() < f32::EPSILON);
        speed.reset();
        assert!((speed.speed() - 1.0).abs() < f32::EPSILON);
        speed.toggle_pause();
        assert_eq!(speed.speed(), 0.0);
        speed.toggle_pause();
        assert!((speed.speed() - 1.0).abs() < f32::EPSILON);
        // Step mentre in pausa: riparte alla nuova velocità.
        speed.toggle_pause();
        speed.step_up();
        assert!((speed.speed() - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn logical_chars_match_any_layout() {
        // Tastiera italiana: '+'/'-' arrivano come caratteri logici anche se
        // i KeyCode fisici sono altrove (Backslash/Slash).
        assert!(logical_char_pressed(
            &Key::Character("+".into()),
            &['+', '=']
        ));
        assert!(logical_char_pressed(
            &Key::Character("-".into()),
            &['-', '_']
        ));
        assert!(logical_char_pressed(&Key::Character("0".into()), &['0']));
        assert!(!logical_char_pressed(
            &Key::Character("a".into()),
            &['+', '=']
        ));
        assert!(!logical_char_pressed(&Key::Enter, &['+', '=']));
    }

    #[test]
    fn team_lines_and_clock_format() {
        let line = format_team_line(&TeamScore {
            label: "BLUE",
            personality: "rusher",
            units: 7,
            army: 5,
            buildings: 3,
            sites: 1,
            income_metal: 5.0,
            stock_metal: 120.4,
            income_energy: 12.0,
            stock_energy: 300.7,
            power: 3450.0,
            orders: 42,
        });
        assert!(line.contains("BLUE rusher"));
        assert!(line.contains("7u (5 army)"));
        assert!(line.contains("3 bldg +1 site"));
        assert!(line.contains("M 5/s [120]"));
        assert!(line.contains("E 12/s [301]"));
        assert!(line.contains("pow 3450"));
        assert!(line.contains("ord 42"));
        assert_eq!(format_game_clock(0.0), "00:00");
        assert_eq!(format_game_clock(75.0), "01:15");
        assert_eq!(format_game_clock(600.0), "10:00");
    }
}
