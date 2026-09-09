use bevy::{
    input::mouse::{MouseScrollUnit, MouseWheel},
    prelude::*,
    window::PrimaryWindow,
};

use crate::{scenario::Scenario, world::GROUND_HALF_SIZE};

#[derive(Resource, Default)]
pub struct CameraFocus(pub Option<Vec3>);

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraFocus>()
            .add_systems(Startup, setup_camera)
            .add_systems(Update, control_camera);
    }
}

#[derive(Component)]
pub struct RtsCamera {
    focus: Vec3,
    yaw: f32,
    distance: f32,
}

impl RtsCamera {
    fn transform(&self) -> Transform {
        let offset = Quat::from_rotation_y(self.yaw) * Vec3::new(0.0, 0.8, 0.6) * self.distance;
        Transform::from_translation(self.focus + offset).looking_at(self.focus, Vec3::Y)
    }
}

fn setup_camera(
    mut commands: Commands,
    scenario: Res<Scenario>,
    view: Option<Res<crate::view::ViewState>>,
) {
    let controller = RtsCamera {
        focus: if matches!(*scenario, Scenario::Playground) {
            scenario.center(view.as_deref().map_or(0, |v| v.team as usize))
                + Vec3::new(0.0, 0.0, 12.0)
        } else {
            Vec3::ZERO
        },
        yaw: 0.0,
        distance: if matches!(*scenario, Scenario::Playground) {
            95.0
        } else {
            440.0
        },
    };
    commands.spawn((Camera3d::default(), controller.transform(), controller));
}

#[allow(clippy::too_many_arguments)]
fn control_camera(
    keys: Res<ButtonInput<KeyCode>>,
    mut wheel: MessageReader<MouseWheel>,
    time: Res<Time<Real>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut camera: Single<(&mut RtsCamera, &mut Transform)>,
    input: Option<Res<crate::ui::industry::MapInput>>,
    interactions: Query<&Interaction, With<crate::ui::industry::BlocksMap>>,
    mut focus: ResMut<CameraFocus>,
) {
    let mut scroll: f32 = wheel
        .read()
        .map(|event| match event.unit {
            MouseScrollUnit::Line => event.y,
            MouseScrollUnit::Pixel => event.y / 40.0,
        })
        .sum();
    if !window.focused {
        return;
    }
    let (controller, transform) = &mut *camera;
    if let Some(point) = focus.0.take() {
        controller.focus = point;
    }
    if input.is_some_and(|i| i.blocked) || interactions.iter().any(|i| *i != Interaction::None) {
        scroll = 0.0;
    }
    let axis = |positive: &[KeyCode], negative: &[KeyCode]| {
        f32::from(keys.any_pressed(positive.iter().copied()))
            - f32::from(keys.any_pressed(negative.iter().copied()))
    };
    let dt = time.delta_secs();
    controller.yaw += axis(&[KeyCode::KeyQ], &[KeyCode::KeyE]) * 1.5 * dt;
    controller.distance = (controller.distance * (-scroll * 0.12).exp()).clamp(15.0, 520.0);
    let local = Vec3::new(
        axis(
            &[KeyCode::KeyD, KeyCode::ArrowRight],
            &[KeyCode::KeyA, KeyCode::ArrowLeft],
        ),
        0.0,
        axis(
            &[KeyCode::KeyS, KeyCode::ArrowDown],
            &[KeyCode::KeyW, KeyCode::ArrowUp],
        ),
    )
    .normalize_or_zero();
    let speed = controller.distance * 0.65;
    let yaw = controller.yaw;
    controller.focus += Quat::from_rotation_y(yaw) * local * speed * dt;
    controller.focus.x = controller
        .focus
        .x
        .clamp(-GROUND_HALF_SIZE, GROUND_HALF_SIZE);
    controller.focus.z = controller
        .focus
        .z
        .clamp(-GROUND_HALF_SIZE, GROUND_HALF_SIZE);
    **transform = controller.transform();
}
