use crate::{
    gui::SimStatistics,
    pheromone::Pheromones,
    utils::{calc_rotation_angle, get_rand_unit_vec2},
    *,
};
use bevy::{
    math::{vec2, vec3},
    prelude::*,
    time::common_conditions::on_timer,
    tasks::{AsyncComputeTaskPool, Task},
};
use futures_lite::future;
use rand::{thread_rng, Rng};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::f32::consts::PI;
use std::time::Duration;

pub struct AntPlugin;

pub enum AntTask {
    FindFood,
    FindHome,
}

#[derive(Component)]
pub struct Ant {
    pub wallet_address: String,
}

#[derive(Component)]
pub struct AntLabel;

#[derive(Component)]
pub struct CurrentTask(pub AntTask);
#[derive(Component)]
struct Velocity(Vec2);
#[derive(Component)]
struct Acceleration(Vec2);
#[derive(Component)]
struct PhStrength(f32);

#[derive(Resource)]
struct AntScanRadius(f32);
#[derive(Resource)]
pub struct AntFollowCameraPos(pub Vec2);

#[derive(Component)]
struct FetchHoldersTask(Task<Vec<String>>);

#[derive(Deserialize)]
struct HeliusTokenAccount {
    address: String,
    mint: String,
    owner: String,
    amount: u64,
    delegated_amount: u64,
    frozen: bool,
}

impl Plugin for AntPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup)
            .insert_resource(AntScanRadius(INITIAL_ANT_PH_SCAN_RADIUS))
            .insert_resource(AntFollowCameraPos(Vec2::ZERO))
            .add_systems(
                Update,
                drop_pheromone.run_if(on_timer(Duration::from_secs_f32(ANT_PH_DROP_INTERVAL))),
            )
            .add_systems(
                Update,
                check_wall_collision.run_if(on_timer(Duration::from_secs_f32(0.1))),
            )
            .add_systems(
                Update,
                check_home_food_collisions.run_if(on_timer(Duration::from_secs_f32(0.1))),
            )
            .add_systems(Update, update_camera_follow_pos)
            .add_systems(
                Update,
                periodic_direction_update.run_if(on_timer(Duration::from_secs_f32(
                    ANT_DIRECTION_UPDATE_INTERVAL,
                ))),
            )
            .add_systems(
                Update,
                update_stats.run_if(on_timer(Duration::from_secs_f32(3.0))),
            )
            .add_systems(
                Update,
                update_scan_radius.run_if(on_timer(Duration::from_secs_f32(1.0))),
            )
            .add_systems(
                Update,
                decay_ph_strength.run_if(on_timer(Duration::from_secs_f32(
                    ANT_PH_STRENGTH_DECAY_INTERVAL,
                ))),
            )
            .add_systems(
                Update,
                start_fetch_holders.run_if(on_timer(Duration::from_secs(HOLDER_FETCH_INTERVAL))),
            )
            .add_systems(Update, handle_fetch_holders_result)
            .add_systems(Update, update_position.after(check_wall_collision))
            .add_systems(Update, update_ant_labels);
    }
}

fn fetch_token_holders() -> Vec<String> {
    let api_key = HELIUS_API_KEY;
    let mint = MINT_ADDRESS;
    let max_pages = MAX_FETCH_PAGES;
    let min_human_balance = MIN_HUMAN_BALANCE;

    let client = reqwest::blocking::Client::new();

    // Get decimals
    let mut decimals: u32 = 0;
    let metadata_body = json!({
        "jsonrpc": "2.0",
        "id": "get-mint-info",
        "method": "getAccountInfo",
        "params": [
            mint,
            {
                "encoding": "jsonParsed"
            }
        ]
    });
    let url = format!("https://mainnet.helius-rpc.com/?api-key={}", api_key);
    if let Ok(response) = client.post(&url).json(&metadata_body).send() {
        if let Ok(json) = response.json::<Value>() {
            if let Some(result) = json.get("result") {
                if let Some(value) = result.get("value") {
                    if let Some(data) = value.get("data") {
                        if let Some(parsed) = data.get("parsed") {
                            if let Some(info) = parsed.get("info") {
                                if let Some(dec) = info.get("decimals") {
                                    if let Some(d) = dec.as_u64() {
                                        decimals = d as u32;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Fetch accounts
    let mut all_owners: HashSet<String> = HashSet::new();
    let mut cursor: Option<String> = None;
    let mut page = 0;
    while page < max_pages {
        page += 1;
        let mut params = json!({
            "mint": mint,
            "limit": 100
        });
        if let Some(c) = cursor.clone() {
            params["cursor"] = json!(c);
        }
        let body = json!({
            "jsonrpc": "2.0",
            "id": "token-accounts-request",
            "method": "getTokenAccounts",
            "params": params
        });
        if let Ok(response) = client.post(&url).json(&body).send() {
            if let Ok(json) = response.json::<Value>() {
                if let Some(result) = json.get("result") {
                    if let Ok(token_accounts) = serde_json::from_value::<Vec<HeliusTokenAccount>>(result["token_accounts"].clone()) {
                        cursor = result["cursor"].as_str().map(|s| s.to_string());
                        let min_raw = if min_human_balance > 0.0 && decimals > 0 {
                            (min_human_balance * 10f64.powf(decimals as f64)) as u64
                        } else {
                            0
                        };
                        for acc in token_accounts {
                            if acc.amount >= min_raw && !acc.frozen {
                                all_owners.insert(acc.owner);
                            }
                        }
                    }
                }
            }
        }
        if cursor.is_none() {
            break;
        }
    }
    
    let holders: Vec<String> = all_owners.into_iter().collect();
    println!("Fetched {} token holders", holders.len());
    holders
}

fn setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    println!("Starting initial holder fetch...");
    let owners = fetch_token_holders();
    println!("Creating {} initial ants", owners.len());
    
    for owner in owners {
        spawn_ant(&mut commands, &asset_server, owner);
    }
}

fn spawn_ant(commands: &mut Commands, asset_server: &Res<AssetServer>, owner: String) {
    let truncated_address = format!("{}...{}", &owner[..4], &owner[owner.len()-4..]);
    
    commands.spawn((
        SpriteBundle {
            texture: asset_server.load(SPRITE_ANT),
            sprite: Sprite {
                color: Color::rgb(1.1, 1.1, 1.0),
                ..default()
            },
            transform: Transform::from_xyz(HOME_LOCATION.0, HOME_LOCATION.1, ANT_Z_INDEX)
                .with_scale(Vec3::splat(ANT_SPRITE_SCALE)),
            ..Default::default()
        },
        Ant { wallet_address: owner },
        CurrentTask(AntTask::FindFood),
        Velocity(get_rand_unit_vec2()),
        Acceleration(Vec2::ZERO),
        PhStrength(ANT_INITIAL_PH_STRENGTH),
    )).with_children(|parent| {
        parent.spawn((
            Text2dBundle {
                text: Text::from_section(
                    truncated_address,
                    TextStyle {
                        font_size: 20.0,
                        color: Color::rgb(1.0, 1.0, 1.0),
                        ..default()
                    },
                ),
                transform: Transform::from_xyz(0.0, 15.0, 1.0),
                ..default()
            },
            AntLabel,
        ));
    });
}

fn start_fetch_holders(mut commands: Commands) {
    let thread_pool = AsyncComputeTaskPool::get();
    let task = thread_pool.spawn(async move {
        fetch_token_holders()
    });
    
    commands.spawn(FetchHoldersTask(task));
}

fn handle_fetch_holders_result(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut tasks: Query<(Entity, &mut FetchHoldersTask)>,
    ant_query: Query<&Ant>,
) {
    for (entity, mut task) in tasks.iter_mut() {
        if let Some(new_owners) = future::block_on(future::poll_once(&mut task.0)) {
            // Remove the task entity
            commands.entity(entity).despawn();
            
            // Get current holders
            let mut current: HashSet<String> = HashSet::new();
            for ant in ant_query.iter() {
                current.insert(ant.wallet_address.clone());
            }
            
            // Count new holders
            let mut new_count = 0;
            
            // Spawn ants for new holders
            for owner in new_owners {
                if !current.contains(&owner) {
                    spawn_ant(&mut commands, &asset_server, owner);
                    new_count += 1;
                }
            }
            
            if new_count > 0 {
                println!("Added {} new ants (holders)", new_count);
            }
        }
    }
}

fn update_ant_labels(
    ant_query: Query<&Transform, With<Ant>>,
    mut label_query: Query<(&mut Transform, &Parent), (With<AntLabel>, Without<Ant>)>,
) {
    for (mut label_transform, parent) in label_query.iter_mut() {
        if let Ok(ant_transform) = ant_query.get(parent.get()) {
            // Keep label upright regardless of ant rotation
            label_transform.rotation = Quat::from_rotation_z(-ant_transform.rotation.to_euler(EulerRot::XYZ).2);
        }
    }
}

fn drop_pheromone(
    mut ant_query: Query<(&Transform, &CurrentTask, &PhStrength), With<Ant>>,
    mut pheromones: ResMut<Pheromones>,
) {
    for (transform, ant_task, ph_strength) in ant_query.iter_mut() {
        let x = transform.translation.x as i32;
        let y = transform.translation.y as i32;

        match ant_task.0 {
            AntTask::FindFood => pheromones.to_home.emit_signal(&(x, y), ph_strength.0),
            AntTask::FindHome => pheromones.to_food.emit_signal(&(x, y), ph_strength.0),
        }
    }
}

fn update_scan_radius(mut scan_radius: ResMut<AntScanRadius>) {
    if scan_radius.0 > INITIAL_ANT_PH_SCAN_RADIUS * ANT_PH_SCAN_RADIUS_SCALE {
        return;
    }

    scan_radius.0 += ANT_PH_SCAN_RADIUS_INCREMENT;
}

fn update_camera_follow_pos(
    ant_query: Query<&Transform, With<Ant>>,
    mut follow_pos: ResMut<AntFollowCameraPos>,
) {
    if let Some(transform) = ant_query.iter().next() {
        follow_pos.0 = transform.translation.truncate();
    }
}

fn update_stats(
    mut stats: ResMut<SimStatistics>,
    scan_radius: Res<AntScanRadius>,
    ant_query: Query<(), With<Ant>>,
) {
    stats.scan_radius = scan_radius.0;
    stats.num_ants = ant_query.iter().len();
}

fn decay_ph_strength(mut ant_query: Query<&mut PhStrength, With<Ant>>) {
    for mut ph_strength in ant_query.iter_mut() {
        ph_strength.0 = f32::max(ph_strength.0 - ANT_PH_STRENGTH_DECAY_RATE, 0.0);
    }
}

fn get_steering_force(target: Vec2, current: Vec2, velocity: Vec2) -> Vec2 {
    let desired = target - current;
    let steering = desired - velocity;
    steering * 0.05
}

fn periodic_direction_update(
    mut ant_query: Query<(&mut Acceleration, &Transform, &CurrentTask, &Velocity), With<Ant>>,
    mut pheromones: ResMut<Pheromones>,
    mut stats: ResMut<SimStatistics>,
    scan_radius: Res<AntScanRadius>,
) {
    (stats.food_cache_size, stats.home_cache_size) = pheromones.clear_cache();

    for (mut acceleration, transform, current_task, velocity) in ant_query.iter_mut() {
        let current_pos = transform.translation;
        let mut target = None;

        // If ant is close to food/home, pull it towards itself
        match current_task.0 {
            AntTask::FindFood => {
                let dist_to_food = transform.translation.distance_squared(vec3(
                    FOOD_LOCATION.0,
                    FOOD_LOCATION.1,
                    0.0,
                ));
                if dist_to_food <= ANT_TARGET_AUTO_PULL_RADIUS * ANT_TARGET_AUTO_PULL_RADIUS {
                    target = Some(vec2(FOOD_LOCATION.0, FOOD_LOCATION.1));
                }
            }
            AntTask::FindHome => {
                let dist_to_home = transform.translation.distance_squared(vec3(
                    HOME_LOCATION.0,
                    HOME_LOCATION.1,
                    0.0,
                ));
                if dist_to_home <= ANT_TARGET_AUTO_PULL_RADIUS * ANT_TARGET_AUTO_PULL_RADIUS {
                    target = Some(vec2(HOME_LOCATION.0, HOME_LOCATION.1));
                }
            }
        };

        if target.is_none() {
            match current_task.0 {
                AntTask::FindFood => {
                    target = pheromones
                        .to_food
                        .get_steer_target(&current_pos, scan_radius.0);
                }
                AntTask::FindHome => {
                    target = pheromones
                        .to_home
                        .get_steer_target(&current_pos, scan_radius.0);
                }
            }
        }

        if target.is_none() {
            // Default direction randomization
            acceleration.0 += get_rand_unit_vec2() * 0.2;
            continue;
        }

        let steering_force = get_steering_force(
            target.unwrap(),
            transform.translation.truncate(),
            velocity.0,
        );

        let mut rng = rand::thread_rng();
        acceleration.0 += steering_force * rng.gen_range(0.4..=ANT_STEERING_FORCE_FACTOR);
    }
}

fn check_home_food_collisions(
    mut ant_query: Query<
        (
            &Transform,
            &mut Sprite,
            &mut Velocity,
            &mut CurrentTask,
            &mut PhStrength,
            &mut Handle<Image>,
        ),
        With<Ant>,
    >,
    asset_server: Res<AssetServer>,
) {
    for (transform, mut sprite, mut velocity, mut ant_task, mut ph_strength, mut image_handle) in
        ant_query.iter_mut()
    {
        // Home collision
        let dist_to_home =
            transform
                .translation
                .distance_squared(vec3(HOME_LOCATION.0, HOME_LOCATION.1, 0.0));
        if dist_to_home < HOME_RADIUS * HOME_RADIUS {
            // rebound only the ants with food
            match ant_task.0 {
                AntTask::FindFood => {}
                AntTask::FindHome => {
                    velocity.0 *= -1.0;
                }
            }
            ant_task.0 = AntTask::FindFood;
            ph_strength.0 = ANT_INITIAL_PH_STRENGTH;
            *image_handle = asset_server.load(SPRITE_ANT);
            sprite.color = Color::rgb(1.0, 1.0, 2.5);
        }

        // Food Collision
        let dist_to_food =
            transform
                .translation
                .distance_squared(vec3(FOOD_LOCATION.0, FOOD_LOCATION.1, 0.0));
        if dist_to_food < FOOD_PICKUP_RADIUS * FOOD_PICKUP_RADIUS {
            match ant_task.0 {
                AntTask::FindFood => {
                    velocity.0 *= -1.0;
                }
                AntTask::FindHome => {}
            }
            ant_task.0 = AntTask::FindHome;
            ph_strength.0 = ANT_INITIAL_PH_STRENGTH;
            *image_handle = asset_server.load(SPRITE_ANT_WITH_FOOD);
            sprite.color = Color::rgb(1.0, 2.0, 1.0);
        }
    }
}

fn check_wall_collision(
    mut ant_query: Query<(&Transform, &Velocity, &mut Acceleration), With<Ant>>,
) {
    for (transform, velocity, mut acceleration) in ant_query.iter_mut() {
        // wall rebound
        let border = 20.0;
        let top_left = (-W / 2.0, H / 2.0);
        let bottom_right = (W / 2.0, -H / 2.0);
        let x_bound = transform.translation.x < top_left.0 + border
            || transform.translation.x >= bottom_right.0 - border;
        let y_bound = transform.translation.y >= top_left.1 - border
            || transform.translation.y < bottom_right.1 + border;
        if x_bound || y_bound {
            let mut rng = thread_rng();
            let target = vec2(rng.gen_range(-200.0..200.0), rng.gen_range(-200.0..200.0));
            acceleration.0 +=
                get_steering_force(target, transform.translation.truncate(), velocity.0);
        }
    }
}

fn update_position(
    mut ant_query: Query<(&mut Transform, &mut Velocity, &mut Acceleration), With<Ant>>,
) {
    for (mut transform, mut velocity, mut acceleration) in ant_query.iter_mut() {
        let old_pos = transform.translation;

        if !acceleration.0.is_nan() {
            velocity.0 = (velocity.0 + acceleration.0).normalize();
            let new_translation =
                transform.translation + vec3(velocity.0.x, velocity.0.y, 0.0) * ANT_SPEED;
            if !new_translation.is_nan() {
                transform.translation = new_translation;
            }
        }

        acceleration.0 = Vec2::ZERO;
        transform.rotation =
            Quat::from_rotation_z(calc_rotation_angle(old_pos, transform.translation) + PI / 2.0);
    }
}