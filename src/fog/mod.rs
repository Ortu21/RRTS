//! Classic RTS fog of war: shroud (never seen) + fog (seen, currently unseen).
//!
//! One [`VisibilityMap`] holds per-team grids over the whole map. Viewers are
//! units ([`sight_range`](crate::units::archetype::sight_range), normally
//! shorter than weapon range) and alive buildings
//! ([`BUILDING_SIGHT`](crate::economy::balance::BUILDING_SIGHT)).
//! Recomputed 4x/second; `explored` is sticky memory, `visible` is live.
//!
//! Rendering (player team only): a single transparent overlay quad dims the
//! terrain, and unseen enemy units/buildings get [`Visibility::Hidden`] so
//! meshes, bars and rings vanish. Selection and attack picks skip hidden
//! entities, so you cannot click what you cannot see.
//!
//! AI usage: read [`VisibilityMap`] directly — it is team-indexed and has no
//! rendering dependency, so scripted or learned opponents query the exact
//! same data the renderer uses for their own team:
//!
//! ```ignore
//! fn ai_targets(
//!     map: Res<VisibilityMap>,
//!     enemies: Query<(&Transform, &Health), (With<Unit>, With<Team>)>,
//!     my_team: u8,
//! ) {
//!     for (transform, health) in &enemies {
//!         // Honest AI: only consider what this team currently sees.
//!         if !map.visible(my_team, transform.translation) {
//!             continue;
//!         }
//!         // ...evaluate target...
//!         let _ = health;
//!     }
//! }
//! ```
//!
//! Limits (MVP): radial sight, no terrain occlusion. Combat targeting is
//! gated on team visibility (acquire + validate + explicit attacks), with an
//! open fallback while a team has no fog data (headless harnesses without
//! the plugin keep legacy behavior).

use crate::{
    combat::Health,
    economy::balance::BUILDING_SIGHT,
    movement::MovementSystems,
    navigation::HALF_SIZE,
    structures::Building,
    units::{Team, Unit, UnitKind, archetype::sight_range},
    view::ViewState,
};
use bevy::prelude::*;
use std::collections::BTreeMap;

/// World size of one fog cell: 4m over a 400m map keeps a 100x100 grid that
/// a full army re-rasterizes in microseconds at 4Hz.
pub const FOG_CELL: f32 = 4.0;
pub const FOG_WIDTH: usize = (HALF_SIZE * 2.0 / FOG_CELL) as usize;
pub const FOG_COUNT: usize = FOG_WIDTH * FOG_WIDTH;
/// Recompute cadence: fresh enough to track armies, sparse enough to cost
/// nothing. Same tick refreshes overlay + concealment, so visuals never lag
/// the data AI reads.
pub const FOG_PERIOD: f32 = 0.25;
/// Overlay darkness: shroud hides everything, fog dims explored ground.
pub const SHROUD_ALPHA: f32 = 0.82;
pub const FOG_ALPHA: f32 = 0.42;
/// Overlay height: above terrain (0.0) and grid lines (0.01), below hulls.
pub const FOG_OVERLAY_Y: f32 = 0.06;

pub struct FogPlugin {
    /// False in headless harnesses: data + concealment still run, only the
    /// overlay mesh is skipped.
    pub render: bool,
}
impl Plugin for FogPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VisibilityMap>()
            .init_resource::<ViewState>()
            .add_systems(Update, update_fog.after(MovementSystems));
        if self.render {
            app.add_systems(Startup, setup_overlay)
                .add_systems(Update, (refresh_overlay, conceal_unseen).after(update_fog));
        } else {
            app.add_systems(Update, conceal_unseen.after(update_fog));
        }
    }
}

/// Per-team fog grids, indexed by team id. `explored` is sticky (shroud vs
/// memory), `visible` is the live snapshot. AI reads these; the renderer
/// derives overlay + concealment from team 0.
#[derive(Resource, Default, Clone)]
pub struct VisibilityMap(pub BTreeMap<u8, TeamFog>);

#[derive(Default, Clone)]
pub struct TeamFog {
    pub explored: Vec<bool>,
    pub visible: Vec<bool>,
}

impl TeamFog {
    fn ensure(&mut self) {
        if self.explored.len() != FOG_COUNT {
            self.explored.resize(FOG_COUNT, false);
        }
        if self.visible.len() != FOG_COUNT {
            self.visible.resize(FOG_COUNT, false);
        }
    }
}

fn cell_index(x: f32, z: f32) -> Option<usize> {
    if !x.is_finite() || !z.is_finite() {
        return None;
    }
    let col = ((x + HALF_SIZE) / FOG_CELL).floor() as isize;
    let row = ((z + HALF_SIZE) / FOG_CELL).floor() as isize;
    if col < 0 || row < 0 || col >= FOG_WIDTH as isize || row >= FOG_WIDTH as isize {
        return None;
    }
    Some(row as usize * FOG_WIDTH + col as usize)
}

fn cell_center(index: usize) -> Vec2 {
    let col = (index % FOG_WIDTH) as f32;
    let row = (index / FOG_WIDTH) as f32;
    Vec2::new(
        -HALF_SIZE + (col + 0.5) * FOG_CELL,
        -HALF_SIZE + (row + 0.5) * FOG_CELL,
    )
}

impl VisibilityMap {
    fn fog(&self, team: u8) -> Option<&TeamFog> {
        self.0.get(&team)
    }
    /// Live visibility for `team` at a world position. Outside the map or
    /// for unknown teams: false (AI sees nothing there).
    pub fn visible(&self, team: u8, pos: Vec3) -> bool {
        cell_index(pos.x, pos.z)
            .and_then(|i| {
                self.fog(team)
                    .map(|f| f.visible.get(i).copied().unwrap_or(false))
            })
            .unwrap_or(false)
    }

    /// Has `team` ever seen this position (explored memory under fog).
    /// Public AI API: e.g. path only through explored ground.
    #[allow(dead_code)]
    pub fn explored(&self, team: u8, pos: Vec3) -> bool {
        cell_index(pos.x, pos.z)
            .and_then(|i| {
                self.fog(team)
                    .map(|f| f.explored.get(i).copied().unwrap_or(false))
            })
            .unwrap_or(false)
    }
}

/// Combat targeting gate: a team may engage what it currently sees. Open
/// (true) when there is no fog data at all — headless combat harnesses and
/// benchmarks run without the plugin and keep legacy behavior. In game the
/// first fog tick registers every team with viewers, so gating is live from
/// then on.
pub fn can_target(map: Option<&VisibilityMap>, team: u8, pos: Vec3) -> bool {
    let Some(map) = map else {
        return true;
    };
    if !map.0.contains_key(&team) {
        return true;
    }
    map.visible(team, pos)
}

/// Stamp one viewer's sight disc. Pure over the map so tests drive it
/// without a world.
fn reveal(map: &mut VisibilityMap, team: u8, at: Vec3, range: f32) {
    if range <= 0.0 || !at.is_finite() {
        return;
    }
    let fog = map.0.entry(team).or_default();
    fog.ensure();
    let lo_col = (((at.x - range + HALF_SIZE) / FOG_CELL).floor() as isize).max(0);
    let hi_col =
        (((at.x + range + HALF_SIZE) / FOG_CELL).ceil() as isize).min(FOG_WIDTH as isize - 1);
    let lo_row = (((at.z - range + HALF_SIZE) / FOG_CELL).floor() as isize).max(0);
    let hi_row =
        (((at.z + range + HALF_SIZE) / FOG_CELL).ceil() as isize).min(FOG_WIDTH as isize - 1);
    let range_sq = range * range;
    for row in lo_row..=hi_row {
        for col in lo_col..=hi_col {
            let index = row as usize * FOG_WIDTH + col as usize;
            let center = cell_center(index);
            if center.distance_squared(at.xz()) <= range_sq {
                fog.visible[index] = true;
            }
        }
    }
}

#[allow(clippy::type_complexity)]
fn update_fog(
    time: Res<Time>,
    mut acc: Local<f32>,
    units: Query<(&Transform, &Team, &UnitKind, &Health), With<Unit>>,
    buildings: Query<(&Transform, &Team, &Health), With<Building>>,
    mut map: ResMut<VisibilityMap>,
) {
    *acc += time.delta_secs();
    if *acc < FOG_PERIOD {
        return;
    }
    *acc = 0.0;
    for fog in map.0.values_mut() {
        fog.ensure();
        fog.visible.fill(false);
    }
    // Boolean-OR rasterization is order-independent, so repeats agree
    // bit-for-bit regardless of ECS chunk order.
    for (transform, team, kind, health) in &units {
        if health.current > 0.0 {
            reveal(&mut map, team.0, transform.translation, sight_range(*kind));
        }
    }
    for (transform, team, health) in &buildings {
        if health.current > 0.0 {
            reveal(&mut map, team.0, transform.translation, BUILDING_SIGHT);
        }
    }
    for fog in map.0.values_mut() {
        for i in 0..FOG_COUNT {
            fog.explored[i] |= fog.visible[i];
        }
    }
}

/// Hide enemy units/buildings the viewed team cannot currently see. Own team
/// is always shown. With fog OFF (spectator) everything stays visible:
/// AI honesty is unaffected (AI reads `VisibilityMap`, never this).
/// Component churn is minimal: only transitions write.
#[allow(clippy::type_complexity)]
fn conceal_unseen(
    mut commands: Commands,
    map: Res<VisibilityMap>,
    view: Res<ViewState>,
    mut hidden: Query<
        (Entity, &Transform, &Team, Option<&mut Visibility>),
        Or<(With<Unit>, With<Building>)>,
    >,
) {
    for (entity, transform, team, visibility) in &mut hidden {
        if team.0 == view.team {
            continue;
        }
        let seen = !view.fog_on || map.visible(view.team, transform.translation);
        let want = if seen {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        match visibility {
            Some(mut current) => {
                if *current != want {
                    *current = want;
                }
            }
            None => {
                if want == Visibility::Hidden {
                    commands.entity(entity).insert(Visibility::Hidden);
                }
            }
        }
    }
}

#[derive(Resource)]
struct FogOverlay {
    mesh: Handle<Mesh>,
}

/// One transparent quad over the whole map, vertex-colored per fog cell:
/// black with shroud/fog alpha, clear where visible. Vertex positions are
/// explicit world coords, so no UV orientation can mirror the fog.
fn fog_mesh() -> Mesh {
    let n = FOG_WIDTH + 1;
    let mut positions = Vec::with_capacity(n * n * 3);
    let mut normals = Vec::with_capacity(n * n * 3);
    let mut uvs = Vec::with_capacity(n * n * 2);
    let mut colors = Vec::with_capacity(n * n * 4);
    for row in 0..n {
        for col in 0..n {
            positions.push([
                -HALF_SIZE + col as f32 * FOG_CELL,
                FOG_OVERLAY_Y,
                -HALF_SIZE + row as f32 * FOG_CELL,
            ]);
            normals.push([0.0, 1.0, 0.0]);
            uvs.push([col as f32 / FOG_WIDTH as f32, row as f32 / FOG_WIDTH as f32]);
            colors.push([0.0, 0.0, 0.0, SHROUD_ALPHA]);
        }
    }
    let mut indices = Vec::with_capacity(FOG_WIDTH * FOG_WIDTH * 6);
    for row in 0..FOG_WIDTH {
        for col in 0..FOG_WIDTH {
            let a = (row * n + col) as u32;
            let b = (row * n + col + 1) as u32;
            let c = ((row + 1) * n + col) as u32;
            let d = ((row + 1) * n + col + 1) as u32;
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    let mut mesh = Mesh::new(
        bevy::render::mesh::PrimitiveTopology::TriangleList,
        bevy::asset::RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(bevy::render::mesh::Indices::U32(indices));
    mesh
}

fn setup_overlay(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mesh = meshes.add(fog_mesh());
    commands.spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::WHITE,
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            cull_mode: None,
            ..default()
        })),
    ));
    commands.insert_resource(FogOverlay { mesh });
}

/// Recolor overlay vertices from the viewed team's fog: vertex (col,row)
/// maps cell (col,row) clamped to the grid, so the mapping is exact by
/// construction. With fog OFF the overlay goes fully transparent.
fn refresh_overlay(
    map: Res<VisibilityMap>,
    view: Res<ViewState>,
    overlay: Res<FogOverlay>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let Some(mut mesh) = meshes.get_mut(&overlay.mesh) else {
        return;
    };
    let n = FOG_WIDTH + 1;
    let fog = view.fog_on.then(|| map.0.get(&view.team)).flatten();
    let Some(fog) = fog else {
        // Spectator without fog, or team with no data yet: clear overlay.
        let colors = vec![[0.0, 0.0, 0.0, 0.0]; n * n];
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        return;
    };
    let mut colors = Vec::with_capacity(n * n * 4);
    for row in 0..n {
        for col in 0..n {
            let cc = col.min(FOG_WIDTH - 1);
            let rr = row.min(FOG_WIDTH - 1);
            let index = rr * FOG_WIDTH + cc;
            let alpha = if fog.visible.get(index).copied().unwrap_or(false) {
                0.0
            } else if fog.explored.get(index).copied().unwrap_or(false) {
                FOG_ALPHA
            } else {
                SHROUD_ALPHA
            };
            colors.push([0.0, 0.0, 0.0, alpha]);
        }
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn revealed(team: u8, at: Vec3, range: f32) -> VisibilityMap {
        let mut map = VisibilityMap::default();
        reveal(&mut map, team, at, range);
        // Explored sticks like the live system does.
        for fog in map.0.values_mut() {
            for i in 0..FOG_COUNT {
                fog.explored[i] |= fog.visible[i];
            }
        }
        map
    }

    #[test]
    fn grid_geometry_covers_map() {
        assert_eq!(FOG_WIDTH, 100);
        assert_eq!(FOG_COUNT, 10_000);
        assert!(cell_index(-HALF_SIZE, -HALF_SIZE).is_some());
        assert!(cell_index(HALF_SIZE - 0.1, HALF_SIZE - 0.1).is_some());
        assert!(cell_index(HALF_SIZE + 1.0, 0.0).is_none());
        assert!(cell_index(f32::NAN, 0.0).is_none());
    }

    #[test]
    fn sight_disc_reveals_and_remembers() {
        let at = Vec3::new(20.0, 0.0, -12.0);
        let map = revealed(0, at, 20.0);
        assert!(map.visible(0, at));
        assert!(map.explored(0, at));
        // Outside the disc: shrouded.
        assert!(!map.visible(0, at + Vec3::X * 40.0));
        assert!(!map.explored(0, at + Vec3::X * 40.0));
        // Memory persists after the viewer leaves: fresh map with no reveal
        // keeps explored from the old one only if carried over — a blank map
        // knows nothing (AI must read the live resource, not snapshots).
        let blank = VisibilityMap::default();
        assert!(!blank.visible(0, at));
    }

    #[test]
    fn teams_are_isolated() {
        let mut map = VisibilityMap::default();
        reveal(&mut map, 0, Vec3::ZERO, 20.0);
        assert!(map.visible(0, Vec3::ZERO));
        assert!(!map.visible(1, Vec3::ZERO));
        assert!(!map.explored(1, Vec3::ZERO));
    }

    #[test]
    fn reveal_is_deterministic_and_edged() {
        let a = revealed(1, Vec3::new(7.0, 0.0, 3.0), 15.0);
        let b = revealed(1, Vec3::new(7.0, 0.0, 3.0), 15.0);
        assert_eq!(a.0[&1].visible, b.0[&1].visible);
        // Edge inclusive: cell center exactly at range stays visible.
        let edge = Vec3::new(15.0, 0.0, 0.0);
        assert!(revealed(0, Vec3::ZERO, 17.0).visible(0, edge));
        // Zero range and garbage reveal nothing and never panic.
        let mut map = VisibilityMap::default();
        reveal(&mut map, 0, Vec3::ZERO, 0.0);
        reveal(&mut map, 0, Vec3::new(f32::INFINITY, 0.0, 0.0), 10.0);
        assert!(!map.visible(0, Vec3::ZERO));
    }

    #[test]
    fn sight_ranges_come_from_archetypes() {
        use crate::{economy::balance::ENGINEER_SIGHT, units::UnitKind};
        assert!(sight_range(UnitKind::Commander) >= sight_range(UnitKind::Tank));
        assert!(sight_range(UnitKind::Engineer) > 0.0);
        assert_eq!(sight_range(UnitKind::Engineer), ENGINEER_SIGHT);
    }

    #[test]
    fn concealment_hides_unseen_enemies_through_the_plugin() {
        use crate::units::{Team, Unit, UnitKind};
        use bevy::time::TimeUpdateStrategy;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                0.3,
            )))
            .add_plugins(FogPlugin { render: false });
        app.finish();
        app.cleanup();
        app.update();
        let blue = app
            .world_mut()
            .spawn((
                Unit(1),
                Team(0),
                UnitKind::Tank,
                Transform::from_translation(Vec3::ZERO),
                Health {
                    current: 120.0,
                    max: 120.0,
                },
            ))
            .id();
        let red = app
            .world_mut()
            .spawn((
                Unit(2),
                Team(1),
                UnitKind::Tank,
                Transform::from_translation(Vec3::X * 100.0),
                Health {
                    current: 120.0,
                    max: 120.0,
                },
            ))
            .id();
        app.update();
        // Far outside tank sight (30m): concealed, own team untouched.
        assert_eq!(
            app.world().get::<Visibility>(red),
            Some(&Visibility::Hidden)
        );
        assert!(app.world().get::<Visibility>(blue).is_none());
        // Walked into sight: revealed again.
        app.world_mut()
            .get_mut::<Transform>(red)
            .unwrap()
            .translation = Vec3::X * 10.0;
        app.update();
        assert_ne!(
            app.world().get::<Visibility>(red),
            Some(&Visibility::Hidden)
        );
    }
}
