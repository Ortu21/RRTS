//! Overlay debug AI (solo grafico): gizmos 3D + pannello punteggio live +
//! controllo velocità demo. Headless non monta il plugin: zero costo di misura.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use super::{AiConfig, AiMode, AiSnapshots, AiState};
use crate::units::{Team, UnitKind};

#[derive(Resource, Default)]
pub struct AiDebugState {
    pub last_intents: Vec<String>,
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

#[derive(Component)]
struct AiScoreHud;

pub struct AiDebugPlugin;
impl Plugin for AiDebugPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AiDebugState>()
            .init_resource::<AiSpeed>()
            .add_systems(Startup, setup_ai_score_hud)
            .add_systems(Update, (draw_snapshots, draw_base_markers))
            .add_systems(Update, ai_speed_keys)
            .add_systems(PostUpdate, (reveal_all_for_spectator, update_ai_score_hud));
    }
}

/// Spettatore con fog OFF: mostra tutto (era il vecchio reveal del modo Both,
/// ora guidato dal toggle FOG). Le AI restano oneste perché leggono
/// `VisibilityMap` per-team, mai `Visibility` di rendering.
/// Corre in PostUpdate, quindi dopo `conceal_unseen` del fog.
#[allow(clippy::type_complexity)]
fn reveal_all_for_spectator(
    view: Res<crate::view::ViewState>,
    config: Res<AiConfig>,
    mut hidden: Query<
        &mut Visibility,
        Or<(With<crate::units::Unit>, With<crate::structures::Building>)>,
    >,
) {
    if config.mode == AiMode::Off || view.fog_on {
        return;
    }
    for mut visibility in &mut hidden {
        if *visibility == Visibility::Hidden {
            *visibility = Visibility::Inherited;
        }
    }
}

fn team_color(team: u8) -> Color {
    if team == 0 {
        Color::srgb(0.3, 0.7, 1.0)
    } else {
        Color::srgb(1.0, 0.4, 0.3)
    }
}

fn draw_snapshots(
    mut gizmos: Gizmos,
    snapshots: Res<AiSnapshots>,
    config: Res<AiConfig>,
    state: Res<AiDebugState>,
) {
    if config.mode == AiMode::Off {
        return;
    }
    // Un overlay per team AI, ordinato per determinismo visivo.
    let mut teams: Vec<u8> = snapshots.0.keys().copied().collect();
    teams.sort_unstable();
    for team in teams {
        let Some(snapshot) = snapshots.0.get(&team) else {
            continue;
        };
        // Anello su ogni unità AI + linea verso nemici visibili (honest view).
        for unit in &snapshot.my_units {
            gizmos.circle(
                Isometry3d::from_translation(unit.pos + Vec3::Y * 0.15),
                1.1,
                team_color(snapshot.team),
            );
        }
        for enemy in snapshot.visible_enemies.iter().take(12) {
            // Linea dal baricentro AI al nemico visto: cosa l'AI "sa".
            if let Some(first) = snapshot.my_units.first() {
                gizmos.line(
                    first.pos + Vec3::Y * 1.0,
                    enemy.pos + Vec3::Y * 1.0,
                    Color::srgb(1.0, 0.85, 0.2),
                );
            }
            gizmos.circle(
                Isometry3d::from_translation(enemy.pos + Vec3::Y * 0.15),
                0.9,
                Color::srgb(1.0, 0.2, 0.2),
            );
        }
        // Marker edifici propri.
        for building in &snapshot.my_buildings {
            let color = if building.under_construction {
                Color::srgb(1.0, 0.6, 0.1)
            } else {
                team_color(snapshot.team)
            };
            gizmos.cube(
                Transform::from_translation(building.pos + Vec3::Y * 1.0)
                    .with_scale(Vec3::new(3.0, 2.0, 3.0)),
                color,
            );
        }
    }
    let _ = &state.last_intents;
}

fn draw_base_markers(
    mut gizmos: Gizmos,
    config: Res<AiConfig>,
    scenario: Res<crate::scenario::Scenario>,
) {
    if config.mode == AiMode::Off {
        return;
    }
    for brain in config.sorted_teams() {
        let home = scenario.center(brain.team as usize);
        let foe = scenario.attack_target(brain.team as usize);
        gizmos.circle(
            Isometry3d::from_translation(home + Vec3::Y * 0.2),
            4.0,
            team_color(brain.team),
        );
        gizmos.line(
            home + Vec3::Y * 2.0,
            foe + Vec3::Y * 2.0,
            Color::srgba(1.0, 1.0, 1.0, 0.25),
        );
    }
}

/// Tasti velocità demo: +/- step, 0 reset 1x, Space pausa/riprendi.
/// Ignorato senza finestra o senza focus (come gli ordini player).
fn ai_speed_keys(
    keys: Res<ButtonInput<KeyCode>>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    mut speed: ResMut<AiSpeed>,
    mut time: ResMut<Time<Virtual>>,
    config: Res<AiConfig>,
) {
    if config.mode == AiMode::Off {
        return;
    }
    if window.is_some_and(|w| !w.focused) {
        return;
    }
    let mut changed = false;
    if keys.just_pressed(KeyCode::Equal) || keys.just_pressed(KeyCode::NumpadAdd) {
        speed.step_up();
        changed = true;
    } else if keys.just_pressed(KeyCode::Minus) || keys.just_pressed(KeyCode::NumpadSubtract) {
        speed.step_down();
        changed = true;
    } else if keys.just_pressed(KeyCode::Digit0)
        || keys.just_pressed(KeyCode::Numpad0)
        || keys.just_pressed(KeyCode::Backspace)
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

fn setup_ai_score_hud(mut commands: Commands) {
    commands.spawn((
        AiScoreHud,
        Interaction::None,
        Text::new("AI battle: warming up..."),
        TextFont {
            font_size: FontSize::Px(15.0),
            ..default()
        },
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            top: px(16),
            right: px(16),
            padding: UiRect::all(px(10)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.03, 0.05, 0.07, 0.85)),
    ));
}

/// Dati punteggio di un team per il pannello live. Struct (non 12 argomenti)
/// così i campi futuri non rompono le firme.
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

/// Pannello live 4Hz su orologio reale (così resta reattivo anche in pausa).
/// In `skirmish` mostra anche il team umano come "YOU".
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_ai_score_hud(
    config: Res<AiConfig>,
    speed: Res<AiSpeed>,
    time: Res<Time<Real>>,
    virtual_time: Res<Time<Virtual>>,
    economy: Option<Res<crate::economy::Economy>>,
    state: Res<AiState>,
    units: Query<(&Team, &UnitKind, &crate::combat::Health), With<crate::units::Unit>>,
    buildings: Query<
        (
            &Team,
            &crate::economy::balance::BuildingKind,
            &crate::combat::Health,
            Has<crate::structures::Construction>,
        ),
        With<crate::structures::Building>,
    >,
    mut hud: Single<&mut Text, With<AiScoreHud>>,
    mut acc: Local<f32>,
) {
    if config.mode == AiMode::Off {
        return;
    }
    *acc += time.delta_secs();
    if *acc < 0.25 {
        return;
    }
    *acc = 0.0;

    let title = match config.mode {
        AiMode::Both => {
            let names: Vec<_> = config
                .sorted_teams()
                .iter()
                .map(|t| format!("{} ({})", team_label(t.team), t.personality.name))
                .collect();
            format!("AI BATTLE {}", names.join(" vs "))
        }
        AiMode::Skirmish | AiMode::Test => {
            let names: Vec<_> = config
                .sorted_teams()
                .iter()
                .map(|t| format!("AI {} ({})", team_label(t.team), t.personality.name))
                .collect();
            format!("{} — you play the other side", names.join(", "))
        }
        AiMode::Off => return,
    };
    let speed_label = if speed.paused {
        "PAUSA".to_owned()
    } else {
        format!("{:.2}x", speed.speed())
    };
    let mut lines = vec![format!(
        "{title} | {} | vel {} [+/- | 0 reset | Space pausa]",
        format_game_clock(virtual_time.elapsed_secs()),
        speed_label,
    )];

    // Team mostrati: entrambi in Both, AI + umano in skirmish.
    let mut shown: Vec<(u8, String)> = config
        .sorted_teams()
        .iter()
        .map(|t| (t.team, t.personality.name.to_owned()))
        .collect();
    if config.mode == AiMode::Skirmish
        && let Some(brain) = config.sorted_teams().first()
    {
        let human = 1 - brain.team.min(1);
        if !shown.iter().any(|(t, _)| *t == human) {
            shown.push((human, "YOU".to_owned()));
        }
        shown.sort_by_key(|(t, _)| *t);
    }

    for (team, personality) in shown {
        let mut unit_count = 0;
        let mut army_count = 0;
        let mut power = 0.0;
        for (t, kind, health) in &units {
            if t.0 != team || health.current <= 0.0 {
                continue;
            }
            unit_count += 1;
            if crate::units::archetype(*kind).armed {
                army_count += 1;
                power += super::strategy::combat_power(*kind, health.current);
            }
        }
        let mut building_count = 0;
        let mut site_count = 0;
        for (t, _, health, site) in &buildings {
            if t.0 != team || health.current <= 0.0 {
                continue;
            }
            if site {
                site_count += 1;
            } else {
                building_count += 1;
            }
        }
        let (stock, income) = economy
            .as_deref()
            .and_then(|e| e.0.get(&team))
            .map(|a| (a.stock, a.income))
            .unwrap_or(([0.0, 0.0], [0.0, 0.0]));
        let orders = state
            .per_team
            .get(&team)
            .map_or(0, |s| s.orders_issued + s.builds_done + s.enqueues_done);
        lines.push(format_team_line(&TeamScore {
            label: team_label(team),
            personality: &personality,
            units: unit_count,
            army: army_count,
            buildings: building_count,
            sites: site_count,
            income_metal: income[0],
            stock_metal: stock[0],
            income_energy: income[1],
            stock_energy: stock[1],
            power,
            orders,
        }));
    }
    hud.0 = lines.join("\n");
}

fn team_label(team: u8) -> &'static str {
    if team == 0 { "BLUE" } else { "RED" }
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
