//! Fine partita: quando un team resta senza Commander vivi, vince l'altro.
//!
//! Solo Playground grafico (nessun Commander nei benchmark → mai trigger).
//! Al game-over: banner centrale, input mappa bloccato, AI fermata (legge
//! `MatchResult`), `R` ricomincia la partita respawnando i Commander.

use crate::{
    combat::Health,
    economy::Economy,
    fog::VisibilityMap,
    navigation::NavGrid,
    orders::PendingOrder,
    scenario::Scenario,
    structures::{Building, Placement},
    units::{Commander, Team, Unit, UnitIds, UnitKind, spawn_combat_unit},
    view::ViewState,
};
use bevy::{prelude::*, window::PrimaryWindow};

/// Esito della partita. `over` blocca ordini player (via `MapInput`) e AI.
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
pub struct MatchResult {
    pub over: bool,
    pub winner: Option<u8>,
    pub draw: bool,
}

impl MatchResult {
    pub fn banner(&self) -> String {
        if !self.over {
            return String::new();
        }
        if self.draw {
            "DRAW — both commanders down\nPress R to restart".into()
        } else if let Some(team) = self.winner {
            format!(
                "{} WINS — enemy commander eliminated\nPress R to restart",
                ViewState::team_name(team)
            )
        } else {
            String::new()
        }
    }
}

/// Puro e testabile: chi vince dati i Commander vivi per team.
/// `seen` è true da quando almeno un Commander è stato visto vivo (evita
/// falsi game-over in scenari senza Commander come i benchmark).
pub fn decide_winner(alive_blue: usize, alive_red: usize, seen: bool) -> MatchResult {
    if !seen {
        return MatchResult::default();
    }
    match (alive_blue, alive_red) {
        (0, 0) => MatchResult {
            over: true,
            winner: None,
            draw: true,
        },
        (0, _) => MatchResult {
            over: true,
            winner: Some(1),
            draw: false,
        },
        (_, 0) => MatchResult {
            over: true,
            winner: Some(0),
            draw: false,
        },
        _ => MatchResult::default(),
    }
}

#[derive(Component)]
struct GameOverBanner;

pub struct GameOverPlugin;
impl Plugin for GameOverPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MatchResult>()
            .init_resource::<ViewState>()
            .add_systems(Startup, setup_banner)
            .add_systems(Update, check_match_end)
            .add_systems(PostUpdate, (update_banner, restart_on_r).chain());
    }
}

fn setup_banner(mut commands: Commands) {
    commands.spawn((
        GameOverBanner,
        Interaction::None,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(34.0),
            ..default()
        },
        TextColor(Color::srgb(1.0, 0.85, 0.3)),
        TextLayout::justify(Justify::Center),
        Node {
            position_type: PositionType::Absolute,
            top: px(180),
            left: px(0),
            right: px(0),
            display: Display::None,
            padding: UiRect::all(px(16)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.02, 0.03, 0.04, 0.75)),
    ));
}

/// Conta i Commander vivi per team ogni 0.25s (orologio reale: funziona anche
/// in pausa demo). Una sola transizione a `over`.
#[allow(clippy::type_complexity)]
fn check_match_end(
    time: Res<Time<Real>>,
    commanders: Query<(&Team, &Health), (With<Unit>, With<Commander>)>,
    mut result: ResMut<MatchResult>,
    mut seen: Local<bool>,
    mut acc: Local<f32>,
) {
    if result.over {
        return;
    }
    *acc += time.delta_secs();
    if *acc < 0.25 {
        return;
    }
    *acc = 0.0;
    let mut alive = [0usize; 2];
    for (team, health) in &commanders {
        if health.current > 0.0 && (team.0 as usize) < 2 {
            alive[team.0 as usize] += 1;
            *seen = true;
        }
    }
    // Nessun Commander mai esistito (benchmark): mai game-over.
    if !*seen && alive == [0, 0] {
        return;
    }
    *result = decide_winner(alive[0], alive[1], *seen);
}

fn update_banner(
    result: Res<MatchResult>,
    mut banner: Single<(&mut Text, &mut Node), With<GameOverBanner>>,
) {
    let (text, node) = &mut *banner;
    if result.over {
        node.display = Display::Flex;
        text.set_if_neq(Text::new(result.banner()));
    } else {
        node.display = Display::None;
    }
}

/// `R` a partita finita: pulisce il campo e respawna i Commander come a
/// inizio Playground. Nav occupancy si auto-ripara (edifici despawnati),
/// economia/fog/AI ripartono da default.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn restart_on_r(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    result: Res<MatchResult>,
    scenario: Res<Scenario>,
    grid: Res<NavGrid>,
    mut ids: ResMut<UnitIds>,
    mut economy: ResMut<Economy>,
    mut visibility: ResMut<VisibilityMap>,
    mut snapshots: Option<ResMut<crate::ai::AiSnapshots>>,
    mut ai_state: Option<ResMut<crate::ai::AiState>>,
    mut memory: Option<ResMut<crate::ai::EnemyMemory>>,
    mut placement: ResMut<Placement>,
    mut pending: ResMut<PendingOrder>,
    field: Query<Entity, Or<(With<Unit>, With<Building>, With<crate::combat::Projectile>)>>,
) {
    let over = result.over;
    if !over || !window.focused || !keys.just_pressed(KeyCode::KeyR) {
        return;
    }
    for entity in &field {
        commands.entity(entity).despawn();
    }
    // Despawn e spawn sono entrambi differiti e applicati in ordine: prima
    // sparisce il vecchio campo, poi nascono i Commander. Le posizioni sono
    // calcolate sulla nav corrente; gli ostacoli degli edifici despawnati
    // spariscono da soli via `sync_occupancy` nei frame successivi.
    *economy = Economy::default();
    *visibility = VisibilityMap::default();
    // Reset AI solo se il plugin AI è attivo (in partita normale senza AI
    // le risorse non esistono: `Option<ResMut>` evita il panic "Resource
    // does not exist" che bloccava `cargo run` liscio).
    if let Some(snapshots) = snapshots.as_deref_mut() {
        *snapshots = crate::ai::AiSnapshots::default();
    }
    if let Some(ai_state) = ai_state.as_deref_mut() {
        *ai_state = crate::ai::AiState::default();
    }
    if let Some(memory) = memory.as_deref_mut() {
        *memory = crate::ai::EnemyMemory::default();
    }
    *placement = Placement::default();
    *pending = PendingOrder::None;
    for team in 0..scenario.teams() {
        let radius = crate::units::archetype(UnitKind::Commander).radius;
        let position = grid.clear_point_for(scenario.center(team), radius);
        spawn_combat_unit(
            &mut commands,
            ids.allocate(),
            Team(team as u8),
            UnitKind::Commander,
            position,
        );
    }
    // Il banner si nasconde al prossimo check (over=false da qui).
    commands.insert_resource(MatchResult::default());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_commanders_anywhere_never_ends() {
        // Benchmark senza Commander: mai game-over.
        assert_eq!(decide_winner(0, 0, false), MatchResult::default());
    }

    #[test]
    fn last_commander_down_decides() {
        assert_eq!(
            decide_winner(0, 1, true),
            MatchResult {
                over: true,
                winner: Some(1),
                draw: false,
            }
        );
        assert_eq!(
            decide_winner(2, 0, true),
            MatchResult {
                over: true,
                winner: Some(0),
                draw: false,
            }
        );
        assert_eq!(
            decide_winner(0, 0, true),
            MatchResult {
                over: true,
                winner: None,
                draw: true,
            }
        );
        assert!(!decide_winner(1, 1, true).over);
    }

    #[test]
    fn banner_text_names_winner() {
        let result = decide_winner(0, 3, true);
        assert!(result.banner().contains("RED WINS"));
        assert!(result.banner().contains('R'));
        assert!(decide_winner(0, 0, true).banner().contains("DRAW"));
        assert!(decide_winner(1, 1, true).banner().is_empty());
    }

    #[test]
    fn plugin_ticks_without_ai_resources() {
        // Regression: `cargo run` liscio (senza `--ai`) andava in panic
        // "Resource does not exist" perché `restart_on_r` chiedeva
        // `ResMut<AiSnapshots/AiState/EnemyMemory>` anche senza `AiPlugin`.
        // Con `Option<ResMut>` deve girare tranquillo anche senza AI.
        use bevy::{input::InputPlugin, window::PrimaryWindow};
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, InputPlugin))
            .insert_resource(Scenario::Playground)
            .init_resource::<NavGrid>()
            .init_resource::<UnitIds>()
            .init_resource::<Economy>()
            .init_resource::<VisibilityMap>()
            .init_resource::<Placement>()
            .init_resource::<PendingOrder>()
            .add_plugins(GameOverPlugin);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        app.finish();
        app.cleanup();
        for _ in 0..5 {
            app.update();
        }
    }
}
