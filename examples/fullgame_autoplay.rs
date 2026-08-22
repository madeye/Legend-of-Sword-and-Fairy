//! Autonomous, headless whole-game route probe.
//!
//! This harness starts a genuine new game, visits active scene triggers using
//! the engine's collision map, confirms menus/dialogue, and auto-battles.  It
//! is intentionally separate from the normal binary: its first job is to
//! discover and validate a route all the way to the ending before the same
//! decisions are used by the recorder.

use rustpal::game_loop::Engine;
use rustpal::global::{
    seed_random, EventObject, ITEMFLAG_USABLE, LOAD_PLAYER_SPRITE, LOAD_SCENE, MAX_PLAYER_ROLES,
};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufWriter, Write};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use winit::keyboard::KeyCode;

const MAX_FRAMES: u64 = 5_000_000;
const IDLE_RESET_FRAMES: u64 = 240;
const DIRECTIONS: [(KeyCode, (i32, i32)); 4] = [
    (KeyCode::ArrowUp, (16, -8)),
    (KeyCode::ArrowRight, (16, 8)),
    (KeyCode::ArrowDown, (-16, 8)),
    (KeyCode::ArrowLeft, (-16, -8)),
];

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct VisitKey {
    scene: u16,
    area: (i32, i32),
    event_id: u16,
    trigger_script: u16,
    trigger_mode: u16,
    state: i16,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct AreaKey {
    scene: u16,
    anchor: (i32, i32),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct TransitionKey {
    area: AreaKey,
    event_id: u16,
}

#[derive(Clone, Copy, Debug)]
struct Target {
    event_id: u16,
    event: EventObject,
    visit: VisitKey,
}

type TouchVisitKey = (u16, (i32, i32), u16, u16, u16);

struct Pilot {
    scene: u16,
    area: (i32, i32),
    last_player: (i32, i32),
    visited: HashSet<VisitKey>,
    visited_touch: HashSet<TouchVisitKey>,
    visited_manual: HashSet<(u16, u16, u16, u16, i16)>,
    failed_targets: HashSet<(u16, u16, u16, u16, i16)>,
    interrupted_targets: HashMap<(u16, u16, u16, u16, i16), u8>,
    deferred: HashSet<VisitKey>,
    transitions: HashMap<TransitionKey, AreaKey>,
    area_visits: HashMap<AreaKey, u32>,
    scene_visits: HashMap<u16, u32>,
    // Touch-trigger scripts may move their own event object while moving the
    // party. Keep the pre-frame positions so a resulting teleport can still
    // be attributed to the portal that actually fired.
    touch_snapshot: Vec<(u16, EventObject)>,
    target: Option<Target>,
    forced_target: Option<Target>,
    path: VecDeque<KeyCode>,
    search_directions: VecDeque<u16>,
    held: Option<KeyCode>,
    idle_frames: u64,
    target_crossings: u8,
    last_scene_escape_frame: u64,
    skip_start_frame: bool,
    scene_steps: u64,
    scene_enter_frame: u32,
}

impl Pilot {
    fn new(engine: &Engine) -> Self {
        let player = player_position(engine);
        let scene = engine.globals.num_scene;
        let area = component_anchor(engine);
        Self {
            scene,
            area,
            last_player: player,
            visited: HashSet::new(),
            visited_touch: HashSet::new(),
            visited_manual: HashSet::new(),
            failed_targets: HashSet::new(),
            interrupted_targets: HashMap::new(),
            deferred: HashSet::new(),
            transitions: HashMap::new(),
            area_visits: HashMap::from([(
                AreaKey {
                    scene,
                    anchor: area,
                },
                1,
            )]),
            scene_visits: HashMap::from([(scene, 1)]),
            touch_snapshot: active_touch_objects(engine),
            target: None,
            forced_target: None,
            path: VecDeque::new(),
            search_directions: VecDeque::new(),
            held: None,
            idle_frames: 0,
            target_crossings: 0,
            last_scene_escape_frame: 0,
            skip_start_frame: false,
            scene_steps: 0,
            scene_enter_frame: engine.globals.frame_num,
        }
    }

    fn release_direction(&mut self, engine: &mut Engine) {
        if let Some(key) = self.held.take() {
            engine.input.handle_key_event(key, false);
        }
    }

    fn nearby_touch_triggers(
        &mut self,
        engine: &Engine,
        scene: u16,
        area: (i32, i32),
        player: (i32, i32),
    ) -> Vec<TransitionKey> {
        let intended = self.target.map(|target| target.event_id);
        let mut candidates = Vec::new();
        for &(event_id, event) in &self.touch_snapshot {
            let trigger_distance = (event.trigger_mode - 4) as i32 * 32 + 16;
            let distance = metric(player, event_position(event));
            if distance > trigger_distance + 32 {
                continue;
            }
            let changed = engine
                .globals
                .game
                .event_objects
                .get(event_id as usize - 1)
                .is_some_and(|live| {
                    live.x != event.x
                        || live.y != event.y
                        || live.trigger_script != event.trigger_script
                        || live.trigger_mode != event.trigger_mode
                        || live.state != event.state
                });
            candidates.push((event_id, event, distance, changed));
        }
        let selected = candidates
            .iter()
            .copied()
            .filter(|(_, _, _, changed)| *changed)
            .min_by_key(|&(event_id, _, distance, _)| (distance, event_id))
            .or_else(|| {
                candidates
                    .iter()
                    .copied()
                    .find(|(event_id, _, _, _)| Some(*event_id) == intended)
            })
            .or_else(|| {
                candidates
                    .into_iter()
                    .min_by_key(|&(event_id, _, distance, _)| (distance, event_id))
            });

        let mut nearby = Vec::new();
        if let Some((event_id, event, _, _)) = selected {
            let visit = VisitKey {
                scene,
                area,
                event_id,
                trigger_script: event.trigger_script,
                trigger_mode: event.trigger_mode,
                state: event.state,
            };
            self.visited.insert(visit);
            self.visited_touch.insert((
                scene,
                area,
                event_id,
                event.trigger_script,
                event.trigger_mode,
            ));
            nearby.push(TransitionKey {
                area: AreaKey {
                    scene,
                    anchor: area,
                },
                event_id,
            });
        }
        nearby
    }

    fn tap(engine: &mut Engine, key: KeyCode) {
        engine.input.handle_key_event(key, true);
        engine.input.update_keyboard_state(engine.ticks() + 1000);
        engine.input.handle_key_event(key, false);
    }

    fn refresh_world(&mut self, engine: &Engine) {
        let scene = engine.globals.num_scene;
        let player = player_position(engine);
        let teleported = scene == self.scene && metric(player, self.last_player) > 128;
        if scene != self.scene || teleported {
            let previous_target = self.target;
            let target_caused_transition = previous_target.is_some_and(|target| {
                if target.event.trigger_mode < 4 {
                    can_search_event_from(
                        self.last_player,
                        event_position(target.event),
                        target.event.trigger_mode.max(1),
                    )
                } else {
                    let trigger_distance = (target.event.trigger_mode - 4) as i32 * 32 + 16;
                    metric(self.last_player, event_position(target.event)) < trigger_distance
                }
            });
            if scene != self.scene && !target_caused_transition {
                if let Some(target) = previous_target {
                    let key = (
                        target.visit.scene,
                        target.event_id,
                        target.event.trigger_script,
                        target.event.trigger_mode,
                        target.event.state,
                    );
                    let interruptions = {
                        let count = self.interrupted_targets.entry(key).or_default();
                        *count = count.saturating_add(1);
                        *count
                    };
                    if interruptions > 4 {
                        // Some plot triggers sit behind a doorway whose script
                        // moves the party before pathfinding can reach the
                        // intended event. Repeatedly abandoning that target
                        // leaves the pilot bouncing through the doorway for
                        // ever. Preserve the intended live trigger and run it
                        // directly when we next return to its scene.
                        self.forced_target = Some(target);
                        eprintln!(
                            "queued unreachable event={} script={} after {} scene-transition interruptions",
                            target.event_id, target.event.trigger_script, interruptions
                        );
                    }
                }
            }
            let from = AreaKey {
                scene: self.scene,
                anchor: self.area,
            };
            let new_area = if scene == self.scene {
                (player.0.div_euclid(64) * 64, player.1.div_euclid(64) * 64)
            } else {
                component_anchor(engine)
            };
            let destination = AreaKey {
                scene,
                anchor: new_area,
            };
            if destination != from {
                if let Some(target) = previous_target {
                    if target_caused_transition {
                        let transition = TransitionKey {
                            area: from,
                            event_id: target.event_id,
                        };
                        self.transitions.insert(transition, destination);
                        self.visited.insert(target.visit);
                        if target.event.trigger_mode >= 4 {
                            self.visited_touch.insert((
                                target.visit.scene,
                                target.visit.area,
                                target.event_id,
                                target.event.trigger_script,
                                target.event.trigger_mode,
                            ));
                        }
                        eprintln!(
                            "learned targeted exit {:?} event={} -> {:?}",
                            from, target.event_id, destination
                        );
                    }
                }
            }
            // The planned path may have crossed an unrelated doorway. Mark
            // the touch trigger close to the pre-transition position, not the
            // interrupted distant target.
            for transition in
                self.nearby_touch_triggers(engine, self.scene, self.area, self.last_player)
            {
                if destination != from {
                    self.transitions.insert(transition, destination);
                    // Reversible maze portals replace their trigger pointer
                    // after firing and move into the destination component.
                    // Count that reverse-facing state as already encountered;
                    // graph selection can still deliberately backtrack via
                    // `choose_least_explored_exit`, but normal target scanning
                    // must not bounce through it immediately forever.
                    if destination.scene == from.scene {
                        if let Some(event) = engine
                            .globals
                            .game
                            .event_objects
                            .get(transition.event_id as usize - 1)
                            .copied()
                            .filter(|event| {
                                event.state > 0
                                    && event.vanish_time == 0
                                    && event.trigger_script != 0
                                    && event.trigger_mode >= 4
                            })
                        {
                            let reverse_visit = VisitKey {
                                scene: destination.scene,
                                area: destination.anchor,
                                event_id: transition.event_id,
                                trigger_script: event.trigger_script,
                                trigger_mode: event.trigger_mode,
                                state: event.state,
                            };
                            self.visited.insert(reverse_visit);
                            self.visited_touch.insert((
                                destination.scene,
                                destination.anchor,
                                transition.event_id,
                                event.trigger_script,
                                event.trigger_mode,
                            ));
                            self.transitions.insert(
                                TransitionKey {
                                    area: destination,
                                    event_id: transition.event_id,
                                },
                                from,
                            );
                            eprintln!("marked destination touch {:?}", reverse_visit);
                        }
                    }
                    eprintln!(
                        "learned exit {:?} event={} -> {:?}",
                        from, transition.event_id, destination
                    );
                } else {
                    eprintln!(
                        "local scripted movement {:?} near event={}",
                        from, transition.event_id
                    );
                }
            }
            if scene != self.scene {
                *self.scene_visits.entry(scene).or_default() += 1;
                eprintln!(
                    "SCENE from={} to={} frame={} steps={} pos={:?}",
                    self.scene,
                    scene,
                    engine.globals.frame_num,
                    self.scene_steps,
                    engine.globals.viewport
                );
                if is_maze_scene(self.scene) {
                    eprintln!(
                        "MAZE leave scene={} map={} steps={} frames={} pos={:?}",
                        self.scene,
                        engine.globals.game.scenes[self.scene as usize - 1].map_num,
                        self.scene_steps,
                        engine
                            .globals
                            .frame_num
                            .saturating_sub(self.scene_enter_frame),
                        engine.globals.viewport
                    );
                }
                self.scene_steps = 0;
                self.scene_enter_frame = engine.globals.frame_num;
                eprintln!(
                    "scene {} -> {} at frame {} position {:?}",
                    self.scene, scene, engine.globals.frame_num, engine.globals.viewport
                );
            } else {
                eprintln!(
                    "area teleport in scene {} at frame {}: {:?} -> {:?}",
                    scene, engine.globals.frame_num, self.last_player, player
                );
            }
            self.scene = scene;
            self.area = new_area;
            *self.area_visits.entry(destination).or_default() += 1;
            self.deferred.retain(|visit| {
                visit.scene != destination.scene || visit.area != destination.anchor
            });
            // Script pointer, mode, and state are part of VisitKey. Preserve
            // completed interactions across room re-entry; real story
            // changes become fresh keys, while the idle retry below still
            // handles the uncommon unchanged-script gate.
            self.target = None;
            self.path.clear();
            self.search_directions.clear();
            if scene == from.scene && !target_caused_transition {
                if let Some(previous) = previous_target {
                    self.target_crossings = self.target_crossings.saturating_add(1);
                    if self.target_crossings > 4 {
                        self.deferred.insert(previous.visit);
                        if let Some(event) = engine
                            .globals
                            .game
                            .event_objects
                            .get(previous.event_id as usize - 1)
                        {
                            self.forced_target = Some(Target {
                                event_id: previous.event_id,
                                event: *event,
                                visit: VisitKey {
                                    scene,
                                    area: self.area,
                                    event_id: previous.event_id,
                                    trigger_script: event.trigger_script,
                                    trigger_mode: event.trigger_mode,
                                    state: event.state,
                                },
                            });
                            eprintln!(
                                "forcing unreachable event={} script={} after {} area crossings",
                                previous.event_id, event.trigger_script, self.target_crossings
                            );
                        }
                        eprintln!(
                            "deferred event={} after {} unrelated area crossings",
                            previous.event_id, self.target_crossings
                        );
                    } else if let Some(event) = engine
                        .globals
                        .game
                        .event_objects
                        .get(previous.event_id as usize - 1)
                        .copied()
                    {
                        let target = Target {
                            event_id: previous.event_id,
                            event,
                            visit: VisitKey {
                                scene,
                                area: self.area,
                                event_id: previous.event_id,
                                trigger_script: event.trigger_script,
                                trigger_mode: event.trigger_mode,
                                state: event.state,
                            },
                        };
                        let exits = self.known_transition_ids();
                        if let Some(mut path) = path_to_event(engine, target, &exits) {
                            if path.len() <= 2 {
                                path.clear();
                            }
                            eprintln!(
                                "continuing target event={} across area {:?} path={}",
                                target.event_id,
                                self.area,
                                path.len()
                            );
                            self.target = Some(target);
                            self.path = path;
                            self.search_directions = VecDeque::from([0, 1, 2, 3]);
                        }
                    }
                }
            } else {
                self.target_crossings = 0;
            }
            self.idle_frames = 0;
        } else if player != self.last_player {
            self.scene_steps += 1;
        }
        self.last_player = player;
        self.touch_snapshot = active_touch_objects(engine);
    }

    fn known_transition_ids(&self) -> HashSet<u16> {
        let area = AreaKey {
            scene: self.scene,
            anchor: self.area,
        };
        self.transitions
            .keys()
            .filter_map(|transition| (transition.area == area).then_some(transition.event_id))
            .collect()
    }

    fn choose_target(&mut self, engine: &Engine) -> Option<(Target, VecDeque<KeyCode>)> {
        let scene = engine.globals.num_scene as usize;
        if scene == 0 || scene >= engine.globals.game.scenes.len() {
            return None;
        }
        let start = engine.globals.game.scenes[scene - 1].event_object_index as usize;
        let end = engine.globals.game.scenes[scene].event_object_index as usize;
        let player = player_position(engine);

        let mut candidates = (start..end)
            .filter_map(|index| {
                let event = engine.globals.game.event_objects[index];
                if event.state <= 0 || event.vanish_time != 0 || event.trigger_script == 0 {
                    return None;
                }
                let event_id = (index + 1) as u16;
                let visit = VisitKey {
                    scene: engine.globals.num_scene,
                    area: self.area,
                    event_id,
                    trigger_script: event.trigger_script,
                    trigger_mode: event.trigger_mode,
                    state: event.state,
                };
                let item_target = story_item_targets_event(engine, event_id);
                if (self.visited.contains(&visit) || self.deferred.contains(&visit)) && !item_target
                {
                    return None;
                }
                if self.failed_targets.contains(&(
                    engine.globals.num_scene,
                    event_id,
                    event.trigger_script,
                    event.trigger_mode,
                    event.state,
                )) {
                    return None;
                }
                if event.trigger_mode < 4
                    && self.visited_manual.contains(&(
                        engine.globals.num_scene,
                        event_id,
                        event.trigger_script,
                        event.trigger_mode,
                        event.state,
                    ))
                    && !item_target
                {
                    return None;
                }
                if event.trigger_mode >= 4
                    && self.visited_touch.contains(&(
                        engine.globals.num_scene,
                        self.area,
                        event_id,
                        event.trigger_script,
                        event.trigger_mode,
                    ))
                {
                    return None;
                }
                if event_id == TOWER_COLLAPSE && !dragon_pillars_cleared(engine) {
                    return None;
                }
                if event_id == HUAXUECHI_ENTRY && !mingwang_defeated(engine) {
                    return None;
                }
                if event_id == YANGZHOU_QIXING_DOOR
                    && (!mingwang_defeated(engine) || dragon_pillars_cleared(engine))
                {
                    return None;
                }
                let pos = event_position(event);
                let distance = metric(player, pos);
                // Exhaust conversations, switches, and chests before taking
                // touch-trigger exits. Among exits, prefer a statically known
                // destination that this retry has not visited yet; otherwise
                // a spawn-adjacent return door can hide every forward route.
                let priority = if item_target {
                    0
                } else if event.trigger_mode < 4 {
                    1
                } else if script_destination_scene(engine, event.trigger_script).is_some_and(
                    |destination| self.scene_visits.get(&destination).copied().unwrap_or(0) == 0,
                ) {
                    2
                } else if script_destination_scene(engine, event.trigger_script).is_none() {
                    3
                } else {
                    4
                };
                Some((
                    priority,
                    distance,
                    Target {
                        event_id,
                        event,
                        visit,
                    },
                ))
            })
            .collect::<Vec<_>>();
        candidates
            .sort_by_key(|&(priority, distance, target)| (priority, distance, target.event_id));
        let exits = self.known_transition_ids();
        for (_, _, target) in candidates {
            if let Some(path) = path_to_event(engine, target, &exits) {
                return Some((target, path));
            }
            if target.event.trigger_mode < 4 {
                if DRAGON_PILLAR_EVENTS.contains(&target.event_id) {
                    // 七星 神龙 fights must be reached on map 185. Firing
                    // 0x0007 from an unreachable spawn skips the maze walk.
                    eprintln!(
                        "skipping unreachable 七星 pillar event={} until a collision path exists",
                        target.event_id
                    );
                    continue;
                }
                eprintln!(
                    "directing unreachable manual event={} script={} mode={} state={}",
                    target.event_id,
                    target.event.trigger_script,
                    target.event.trigger_mode,
                    target.event.state
                );
                return Some((target, VecDeque::new()));
            }
            self.deferred.insert(target.visit);
        }
        None
    }

    fn choose_least_explored_exit(&self, engine: &Engine) -> Option<(Target, VecDeque<KeyCode>)> {
        let area = AreaKey {
            scene: self.scene,
            anchor: self.area,
        };
        let scene = self.scene as usize;
        if scene == 0 || scene >= engine.globals.game.scenes.len() {
            return None;
        }
        let first = engine.globals.game.scenes[scene - 1].event_object_index as usize;
        let last = engine.globals.game.scenes[scene].event_object_index as usize;
        let exits = self.known_transition_ids();
        let mut candidates = self
            .transitions
            .iter()
            .filter_map(|(transition, destination)| {
                if transition.area != area {
                    return None;
                }
                let index = transition.event_id as usize - 1;
                if index < first || index >= last {
                    return None;
                }
                let event = engine.globals.game.event_objects[index];
                if event.state <= 0
                    || event.vanish_time != 0
                    || event.trigger_script == 0
                    || event.trigger_mode < 4
                {
                    return None;
                }
                if transition.event_id == TOWER_COLLAPSE && !dragon_pillars_cleared(engine) {
                    return None;
                }
                if transition.event_id == HUAXUECHI_ENTRY && !mingwang_defeated(engine) {
                    return None;
                }
                let target = Target {
                    event_id: transition.event_id,
                    event,
                    visit: VisitKey {
                        scene: self.scene,
                        area: self.area,
                        event_id: transition.event_id,
                        trigger_script: event.trigger_script,
                        trigger_mode: event.trigger_mode,
                        state: event.state,
                    },
                };
                let path = path_to_event(engine, target, &exits)?;
                let visits = self.area_visits.get(destination).copied().unwrap_or(0);
                Some((visits, path.len(), target.event_id, target, path))
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|&(visits, distance, event_id, _, _)| (visits, distance, event_id));
        candidates
            .into_iter()
            .map(|(_, _, _, target, path)| (target, path))
            .next()
    }

    fn choose_scene_escape(&self, engine: &Engine, idle_proven: bool) -> Option<Target> {
        let scene_visits = self
            .area_visits
            .iter()
            .filter_map(|(area, visits)| (area.scene == self.scene).then_some(*visits))
            .sum::<u32>();
        if scene_visits < 64 && !idle_proven {
            return None;
        }

        let scene = self.scene as usize;
        if scene == 0 || scene >= engine.globals.game.scenes.len() {
            return None;
        }
        let first = engine.globals.game.scenes[scene - 1].event_object_index as usize;
        let last = engine.globals.game.scenes[scene].event_object_index as usize;
        let mut exits = (first..last)
            .filter_map(|index| {
                let event = engine.globals.game.event_objects[index];
                if event.state <= 0
                    || event.vanish_time != 0
                    || event.trigger_script == 0
                    || event.trigger_mode < 4
                {
                    return None;
                }
                let destination = script_destination_scene(engine, event.trigger_script)?;
                if destination == self.scene {
                    return None;
                }
                let event_id = (index + 1) as u16;
                if event_id == TOWER_COLLAPSE && !dragon_pillars_cleared(engine) {
                    return None;
                }
                if event_id == HUAXUECHI_ENTRY && !mingwang_defeated(engine) {
                    return None;
                }
                Some((
                    self.scene_visits.get(&destination).copied().unwrap_or(0),
                    event_id,
                    Target {
                        event_id,
                        event,
                        visit: VisitKey {
                            scene: self.scene,
                            area: self.area,
                            event_id,
                            trigger_script: event.trigger_script,
                            trigger_mode: event.trigger_mode,
                            state: event.state,
                        },
                    },
                ))
            })
            .collect::<Vec<_>>();
        exits.sort_by_key(|&(visits, event_id, _)| (visits, event_id));
        exits.into_iter().map(|(_, _, target)| target).next()
    }

    fn enter_qixing_array(&mut self, engine: &mut Engine) {
        eprintln!(
            "entering 七星磐龙阵 via script={} from scene={}",
            QIXING_WARP, engine.globals.num_scene
        );
        engine.run_trigger_script(QIXING_WARP, 0xFFFF);
        // 29171 is only opcode 0x0059. Scene 144's enter script (29174)
        // places the party with 0x0046; that runs inside start_frame after
        // the map chunk is loaded. Pathfinding before that enter still uses
        // 化血池 coordinates and treats every pillar as unreachable.
        engine.load_resources();
        if engine.globals.entering_scene {
            engine.start_frame();
        }
        self.target = None;
        self.forced_target = None;
        self.path.clear();
        self.search_directions.clear();
        self.skip_start_frame = true;
    }

    fn maybe_enter_qixing_array(&mut self, engine: &mut Engine) -> bool {
        if !mingwang_defeated(engine) || dragon_pillars_cleared(engine) {
            return false;
        }
        if engine.globals.num_scene != 138 {
            return false;
        }
        // Scene 138 is 化血池. The collapse touch (2417) must not fire until
        // the seven 神龙 on scene 144 have been fought through 0x0007.
        self.enter_qixing_array(engine);
        true
    }

    fn prepare_frame(&mut self, engine: &mut Engine) {
        self.skip_start_frame = false;
        self.release_direction(engine);
        self.refresh_world(engine);

        if engine.globals.entering_scene {
            // Opcode 0x0059 sets entering_scene; the enter script (party
            // 0x0046, set-party, dialog) runs inside start_frame after
            // load_resources. Targeting against the previous viewport would
            // treat every object as unreachable and fire search scripts
            // without walking the new collision map.
            eprintln!(
                "waiting for scene {} enter script before pathfinding",
                engine.globals.num_scene
            );
            return;
        }

        if self.scene == 17 && smash_next_statue(engine) {
            self.target = None;
            self.path.clear();
            self.search_directions.clear();
            self.skip_start_frame = true;
            return;
        }

        if self.scene == 272 && place_next_altar_item(engine) {
            self.target = None;
            self.path.clear();
            self.search_directions.clear();
            self.skip_start_frame = true;
            return;
        }

        let puppet_worms = inventory_amount(engine, 152);
        if puppet_worms >= 36 {
            match self.scene {
                // Once the requested 36 puppet worms have been earned, use
                // the equipped Earth Spirit Pearl through its real item
                // script. Its ordinary failure branch executes opcode 0x0038
                // and returns the party to the Trial Cave entrance.
                216..=226 => {
                    let script = engine.globals.game.objects[267].item_script_on_use();
                    eprintln!(
                        "trial cave complete with {} puppet worms; using Earth Spirit Pearl script={}",
                        puppet_worms, script
                    );
                    engine.run_trigger_script(script, 0xFFFF);
                    self.target = None;
                    self.path.clear();
                    self.skip_start_frame = true;
                    return;
                }
                // Leave the cave entrance for the Dali outskirts, then take
                // the live road trigger back to the Sage's house.
                215 => {
                    let script = engine.globals.game.scenes[214].script_on_teleport;
                    eprintln!(
                        "leaving Trial Cave entrance with {} puppet worms via script={}",
                        puppet_worms, script
                    );
                    engine.run_trigger_script(script, 0xFFFF);
                    self.target = None;
                    self.path.clear();
                    self.skip_start_frame = true;
                    return;
                }
                214 => {
                    let event_id = 3876u16;
                    let index = event_id as usize - 1;
                    let script = engine.globals.game.event_objects[index].trigger_script;
                    if script == 35290 {
                        eprintln!(
                            "returning {} puppet worms to the Sage via event={} script={}",
                            puppet_worms, event_id, script
                        );
                        let next = engine.run_trigger_script(script, event_id);
                        engine.globals.game.event_objects[index].trigger_script = next;
                        self.target = None;
                        self.path.clear();
                        self.skip_start_frame = true;
                        return;
                    }
                    // The same road object is repointed after Ling'er
                    // rejoins. Normal late-game navigation handles its new
                    // destination below.
                }
                _ => {}
            }
        }

        if self.target.is_none()
            && u64::from(engine.globals.frame_num)
                >= self
                    .last_scene_escape_frame
                    .saturating_add(IDLE_RESET_FRAMES)
        {
            if let Some(target) = self.choose_scene_escape(engine, false) {
                let index = target.event_id as usize - 1;
                let script = engine.globals.game.event_objects[index].trigger_script;
                eprintln!(
                    "escaping closed scene loop through event={} script={} destination={:?}",
                    target.event_id,
                    script,
                    script_destination_scene(engine, script)
                );
                let next = engine.run_trigger_script(script, target.event_id);
                engine.globals.game.event_objects[index].trigger_script = next;
                self.target = None;
                self.path.clear();
                self.search_directions.clear();
                self.last_scene_escape_frame = u64::from(engine.globals.frame_num);
                self.skip_start_frame = true;
                return;
            }
        }

        if let Some(target) = self.forced_target.take() {
            if target.visit.scene != engine.globals.num_scene {
                self.forced_target = Some(target);
            } else if target.event_id == TOWER_COLLAPSE && !dragon_pillars_cleared(engine) {
                eprintln!(
                    "deferring tower collapse event={} until 七星磐龙阵 pillars are fought",
                    target.event_id
                );
                self.enter_qixing_array(engine);
                return;
            } else if target.event_id == HUAXUECHI_ENTRY && !mingwang_defeated(engine) {
                eprintln!(
                    "deferring 化血池 exit event={} until 镇狱明王 is defeated",
                    target.event_id
                );
            } else {
                let index = target.event_id as usize - 1;
                let script = engine.globals.game.event_objects[index].trigger_script;
                eprintln!(
                    "running forced event={} script={} destination={:?}",
                    target.event_id,
                    script,
                    script_destination_scene(engine, script)
                );
                let next = engine.run_trigger_script(script, target.event_id);
                engine.globals.game.event_objects[index].trigger_script = next;
                self.visited.insert(target.visit);
                self.target = None;
                self.target_crossings = 0;
                self.skip_start_frame = true;
                return;
            }
        }

        if self.maybe_enter_qixing_array(engine) {
            return;
        }

        if self.target.is_none() {
            let choice = self
                .choose_target(engine)
                .or_else(|| self.choose_least_explored_exit(engine));
            if let Some((target, planned)) = choice {
                self.target_crossings = 0;
                self.target = Some(target);
                self.path = planned;
                self.search_directions = VecDeque::from([0, 1, 2, 3]);
                eprintln!(
                    "target scene={} area={:?} event={} script={} mode={} state={} player={:?} pos={:?} path={}",
                    engine.globals.num_scene,
                    self.area,
                    target.event_id,
                    target.event.trigger_script,
                    target.event.trigger_mode,
                    target.event.state,
                    player_position(engine),
                    event_position(target.event),
                    self.path.len()
                );
            }
        }

        let Some(target) = self.target else {
            self.idle_frames += 1;
            // Let auto scripts advance. If the scene stays quiescent, allow a
            // fresh pass because a previously visited NPC may be required
            // again even when its script pointer did not change.
            if self.idle_frames >= IDLE_RESET_FRAMES {
                if let Some(target) = self.choose_scene_escape(engine, true) {
                    eprintln!(
                        "forcing scene exit event={} after a fully idle pass",
                        target.event_id
                    );
                    self.forced_target = Some(target);
                    self.last_scene_escape_frame = u64::from(engine.globals.frame_num);
                    self.idle_frames = 0;
                    return;
                }
                // Overworld maps do not represent their road exits as event
                // objects. Walking beyond the map boundary executes the
                // scene's teleport script (opcode 0x0038); once every live
                // object has been exhausted, invoke that same script so the
                // offscreen pilot can leave the map without blindly walking
                // every boundary tile.
                let teleport_script = engine.globals.game.scenes
                    [engine.globals.num_scene as usize - 1]
                    .script_on_teleport;
                if teleport_script != 0 {
                    eprintln!(
                        "running scene {} boundary teleport script={} destination={:?} after a fully idle pass",
                        self.scene,
                        teleport_script,
                        script_destination_scene(engine, teleport_script)
                    );
                    engine.run_trigger_script(teleport_script, 0xFFFF);
                    self.idle_frames = 0;
                    self.skip_start_frame = true;
                    return;
                }
                let scene = self.scene;
                let area = self.area;
                self.visited.retain(|visit| {
                    if visit.scene != scene || visit.area != area {
                        return true;
                    }
                    engine
                        .globals
                        .game
                        .event_objects
                        .get(visit.event_id as usize - 1)
                        .is_some_and(|event| event.trigger_mode >= 4)
                });
                self.deferred
                    .retain(|visit| visit.scene != scene || visit.area != area);
                self.idle_frames = 0;
                eprintln!(
                    "retry scene {} area {:?} after an idle pass",
                    self.scene, self.area
                );
            }
            return;
        };

        self.idle_frames = 0;
        if let Some(key) = self.path.pop_front() {
            engine.input.handle_key_event(key, true);
            engine.input.update_keyboard_state(engine.ticks() + 1000);
            self.held = Some(key);
            return;
        }

        // At an engine-valid manual interaction point, invoke the same trigger
        // script that PAL_Search would select. This avoids lower-numbered
        // overlapping scenery objects stealing a synthetic Search press while
        // retaining the real dialogue, animation, battle, and scene logic.
        if target.event.trigger_mode < 4 {
            let index = target.event_id as usize - 1;
            try_story_items(engine, target);
            if target.event.trigger_mode == 0 {
                // Mode-zero scenery is not searchable. Story items can still
                // activate it (the Xianling Island hammer/statue puzzle is the
                // first required case), so using applicable items is the
                // complete interaction for this visit.
                self.visited.insert(target.visit);
                self.visited_manual.insert((
                    target.visit.scene,
                    target.event_id,
                    target.event.trigger_script,
                    target.event.trigger_mode,
                    target.event.state,
                ));
                self.target = None;
                self.skip_start_frame = true;
                return;
            }
            let script = engine.globals.game.event_objects[index].trigger_script;
            let next = engine.run_trigger_script(script, target.event_id);
            engine.globals.game.event_objects[index].trigger_script = next;
            self.visited.insert(target.visit);
            self.visited_manual.insert((
                target.visit.scene,
                target.event_id,
                target.event.trigger_script,
                target.event.trigger_mode,
                target.event.state,
            ));
            self.target = None;
            // A trigger can request a scene-resource reload. Let the outer
            // loop service it before processing another gameplay frame.
            self.skip_start_frame = true;
            return;
        }

        // A touch-trigger exit may be blocked by a manual NPC. Probe all
        // directions from the closest reachable point before deferring it.
        if let Some(direction) = self.search_directions.pop_front() {
            engine.globals.party_direction = direction;
            Self::tap(engine, KeyCode::Space);
            return;
        }

        Self::tap(engine, KeyCode::Space);
        let live = engine.globals.game.event_objects[target.event_id as usize - 1];
        let trigger_distance = ((live.trigger_mode.saturating_sub(4)) as i32 * 32 + 16).max(16);
        let activated = metric(player_position(engine), event_position(live)) < trigger_distance
            || live.trigger_script != target.visit.trigger_script
            || live.state != target.visit.state;
        if activated {
            self.visited.insert(target.visit);
            self.visited_touch.insert((
                self.scene,
                self.area,
                target.event_id,
                live.trigger_script,
                live.trigger_mode,
            ));
        } else {
            // The collision map can leave the nearest reachable tile just
            // outside an event's trigger radius (scene 47's sole forward exit
            // is one such case). We have already exhausted every approach and
            // interaction direction, so execute the still-live trigger rather
            // than permanently abandoning the only route onward.
            self.forced_target = Some(Target {
                event_id: target.event_id,
                event: live,
                visit: VisitKey {
                    scene: self.scene,
                    area: self.area,
                    event_id: target.event_id,
                    trigger_script: live.trigger_script,
                    trigger_mode: live.trigger_mode,
                    state: live.state,
                },
            });
            eprintln!(
                "forcing unreached touch target scene={} event={} script={} player={:?} pos={:?}",
                self.scene,
                target.event_id,
                live.trigger_script,
                player_position(engine),
                event_position(live)
            );
        }
        self.target = None;
    }
}

fn is_maze_scene(scene: u16) -> bool {
    matches!(
        scene,
        16 | 17
            | 40..=47
            | 70..=74
            | 100..=113
            | 144
            | 172..=199
            | 215..=226
            | 277..=293
    )
}

/// 锁妖塔 镇狱明王 (scene 139 event 2422). After this fight the story
/// requires 七星磐龙阵 (scene 144, seven 神龙 0x0007) before the tower
/// collapse (event 2417 / scenes 140–143).
const MINGWANG_EVENT: u16 = 2422;
const QIXING_WARP: u16 = 29171; // opcode 0x0059 to scene 144
const HUAXUECHI_ENTRY: u16 = 2421; // 139 → 138 化血池
const TOWER_COLLAPSE: u16 = 2417; // 138 → 140 collapse cutscene
const YANGZHOU_QIXING_DOOR: u16 = 2561; // scene 152 search warp to 144
const DRAGON_PILLAR_EVENTS: [u16; 7] = [2466, 2467, 2468, 2469, 2470, 2471, 2472];

fn mingwang_defeated(engine: &Engine) -> bool {
    engine
        .globals
        .game
        .event_objects
        .get(MINGWANG_EVENT as usize - 1)
        .is_some_and(|event| event.state <= 0)
}

fn dragon_pillars_cleared(engine: &Engine) -> bool {
    DRAGON_PILLAR_EVENTS.iter().all(|&event_id| {
        engine
            .globals
            .game
            .event_objects
            .get(event_id as usize - 1)
            .is_some_and(|event| event.state != 2)
    })
}

fn player_position(engine: &Engine) -> (i32, i32) {
    (
        engine.globals.viewport.0 + engine.globals.partyoffset.0,
        engine.globals.viewport.1 + engine.globals.partyoffset.1,
    )
}

fn inventory_amount(engine: &Engine, item: u16) -> u16 {
    engine
        .globals
        .inventory
        .iter()
        .find_map(|entry| (entry.item == item).then_some(entry.amount))
        .unwrap_or(0)
}

fn item_is_equipped(engine: &Engine, item: u16) -> bool {
    engine
        .globals
        .game
        .player_roles
        .equipment
        .iter()
        .flatten()
        .any(|&equipped| equipped == item)
}

fn make_item_available(engine: &mut Engine, item: u16) {
    if inventory_amount(engine, item) > 0 {
        return;
    }
    let mut unequipped = false;
    for equipment in &mut engine.globals.game.player_roles.equipment {
        for equipped in equipment {
            if *equipped == item {
                *equipped = 0;
                unequipped = true;
            }
        }
    }
    // These unique plot items were acquired earlier in the route. A party
    // roster transition in the DOS save layout can leave an inactive role's
    // accessory outside both the active equipment table and inventory; make
    // it selectable again at its one legitimate story gate.
    if unequipped || !item_is_equipped(engine, item) {
        engine.globals.add_item_to_inventory(item, 1);
        engine.update_equipments();
    }
}

fn smash_next_statue(engine: &mut Engine) -> bool {
    // The Xianling Island hammer is one stateful item script: each successful
    // use advances to the script for the next statue. The general explorer can
    // legitimately trigger one of the statue objects directly, leaving that
    // pointer behind the world state. Re-synchronize it with the first live
    // statue and perform the normal item-use script from the adjacent tile.
    let statues = [
        (238u16, 39645u16),
        (239, 39647),
        (240, 39649),
        (241, 39651),
        (242, 39653),
        (243, 39655),
    ];
    for (event_id, script) in statues {
        let index = event_id as usize - 1;
        let event = engine.globals.game.event_objects[index];
        if event.state <= 0 {
            continue;
        }

        let hammer = 279u16;
        make_item_available(engine, hammer);
        engine.globals.game.objects[hammer as usize].set_item_script_on_use(script);
        let position = event_position(event);
        engine.globals.party_direction = 0;
        engine.globals.viewport = (
            position.0 + 16 - engine.globals.partyoffset.0,
            position.1 - 8 - engine.globals.partyoffset.1,
        );
        eprintln!(
            "smashing Xianling statue event={} state={} with hammer script={}",
            event_id, event.state, script
        );
        let next = engine.run_trigger_script(script, 0xFFFF);
        engine.globals.game.objects[hammer as usize].set_item_script_on_use(next);
        if engine.script.script_success {
            let trigger = engine.globals.game.event_objects[index].trigger_script;
            let next_trigger = engine.run_trigger_script(trigger, event_id);
            engine.globals.game.event_objects[index].trigger_script = next_trigger;
            eprintln!(
                "smashed Xianling statue event={} via trigger={} new_state={}",
                event_id, trigger, engine.globals.game.event_objects[index].state
            );
        }
        return true;
    }
    false
}

fn place_next_altar_item(engine: &mut Engine) -> bool {
    // Sacred Spirit Pearl reveals the five slots. The elemental pearl item
    // scripts then target these exact event objects and trigger the rain
    // cutscene when the final slot reaches its placed state.
    let placements = [
        (260u16, 4923u16, 0i16),
        (263, 4925, 3),
        (264, 4927, 3),
        (265, 4929, 3),
        (266, 4926, 3),
        (267, 4928, 3),
    ];
    for (item, event_id, placed_state) in placements {
        let index = event_id as usize - 1;
        let event = engine.globals.game.event_objects[index];
        let already_placed = if item == 260 {
            event.state == placed_state
        } else {
            event.state >= placed_state
        };
        if already_placed {
            continue;
        }

        make_item_available(engine, item);
        let position = event_position(event);
        engine.globals.party_direction = 0;
        engine.globals.viewport = (
            position.0 + 16 - engine.globals.partyoffset.0,
            position.1 - 8 - engine.globals.partyoffset.1,
        );
        let script = engine.globals.game.objects[item as usize].item_script_on_use();
        eprintln!(
            "placing altar item={} on event={} state={} with script={}",
            item, event_id, event.state, script
        );
        let next = engine.run_trigger_script(script, 0xFFFF);
        engine.globals.game.objects[item as usize].set_item_script_on_use(next);
        return true;
    }
    false
}

fn event_position(event: EventObject) -> (i32, i32) {
    (event.x as i16 as i32, event.y as i16 as i32)
}

fn active_touch_objects(engine: &Engine) -> Vec<(u16, EventObject)> {
    let scene = engine.globals.num_scene as usize;
    if scene == 0 || scene >= engine.globals.game.scenes.len() {
        return Vec::new();
    }
    let first = engine.globals.game.scenes[scene - 1].event_object_index as usize;
    let last = engine.globals.game.scenes[scene].event_object_index as usize;
    (first..last)
        .filter_map(|index| {
            let event = engine.globals.game.event_objects[index];
            (event.state > 0
                && event.vanish_time == 0
                && event.trigger_script != 0
                && event.trigger_mode >= 4)
                .then_some(((index + 1) as u16, event))
        })
        .collect()
}

fn arm_party_for_route_probe(engine: &mut Engine) {
    let roles = &mut engine.globals.game.player_roles;
    for role in 0..MAX_PLAYER_ROLES {
        // Keep stats inside the DOS WORD/i16-safe band used by battle math.
        // 30k HP / 5k ATK+DEF makes 比武 (team 188) fail to stick damage
        // across turns, so the scripted boss heals back to full and wipes
        // the party; 9999/2000 matches the instant-battle refill and wins.
        roles.max_hp[role] = roles.max_hp[role].max(9999);
        roles.hp[role] = roles.max_hp[role];
        roles.max_mp[role] = roles.max_mp[role].max(9999);
        roles.mp[role] = roles.max_mp[role];
        roles.attack_strength[role] = roles.attack_strength[role].max(2000);
        roles.magic_strength[role] = roles.magic_strength[role].max(2000);
        roles.defense[role] = roles.defense[role].max(400);
        roles.dexterity[role] = roles.dexterity[role].max(300);
        roles.poison_resistance[role] = 100;
    }
}

fn try_story_items(engine: &mut Engine, target: Target) -> bool {
    let player = player_position(engine);
    let event = event_position(target.event);
    let facing_points = [
        (0u16, (event.0 + 16, event.1 - 8)),
        (1u16, (event.0 + 16, event.1 + 8)),
        (2u16, (event.0 - 16, event.1 + 8)),
        (3u16, (event.0 - 16, event.1 - 8)),
    ];
    let preferred_direction = facing_points
        .into_iter()
        .min_by_key(|&(_, point)| metric(player, point))
        .map(|(direction, _)| direction)
        .unwrap_or(0);
    let mut directions = vec![preferred_direction];
    directions.extend((0..4).filter(|&direction| direction != preferred_direction));

    let items = engine
        .globals
        .inventory
        .iter()
        .filter_map(|entry| {
            if entry.item == 0 || entry.amount == 0 {
                return None;
            }
            let object = engine.globals.game.objects[entry.item as usize];
            let flags = object.item_flags();
            let script = object.item_script_on_use();
            let targets_event = engine
                .globals
                .game
                .script_entries
                .get(script as usize)
                .is_some_and(|entry| {
                    entry.operation == 0x0081 && entry.operand[0] == target.event_id
                });
            (flags & ITEMFLAG_USABLE != 0 && script != 0 && targets_event).then_some(entry.item)
        })
        .collect::<Vec<_>>();

    for item in items {
        for &direction in &directions {
            engine.globals.party_direction = direction;
            let point = facing_points[direction as usize];
            if !can_search_event_from(player_position(engine), event, 1) {
                // Item opcode 0x0081 requires the party to be immediately
                // adjacent and facing this exact event object. Some required
                // targets are intentionally behind blockers or maze portals;
                // pathfinding has already proved them unreachable, so place
                // the offscreen pilot at the matching interaction point.
                engine.globals.viewport = (
                    point.1 .0 - engine.globals.partyoffset.0,
                    point.1 .1 - engine.globals.partyoffset.1,
                );
            }
            let event_index = target.event_id as usize - 1;
            let before = engine.globals.game.event_objects[event_index];
            let script = engine.globals.game.objects[item as usize].item_script_on_use();
            let next = engine.run_trigger_script(script, 0xFFFF);
            engine.globals.game.objects[item as usize].set_item_script_on_use(next);
            let after = engine.globals.game.event_objects[event_index];
            if engine.script.script_success
                && (after.trigger_script != before.trigger_script
                    || after.trigger_mode != before.trigger_mode
                    || after.state != before.state)
            {
                eprintln!(
                    "used story item={} on event={} facing={} script {} -> {} mode {} -> {} state {} -> {}",
                    item,
                    target.event_id,
                    direction,
                    before.trigger_script,
                    after.trigger_script,
                    before.trigger_mode,
                    after.trigger_mode,
                    before.state,
                    after.state
                );
                return true;
            }
        }
    }
    false
}

fn story_item_targets_event(engine: &Engine, event_id: u16) -> bool {
    engine.globals.inventory.iter().any(|entry| {
        if entry.item == 0 || entry.amount == 0 {
            return false;
        }
        let object = engine.globals.game.objects[entry.item as usize];
        let flags = object.item_flags();
        if flags & ITEMFLAG_USABLE == 0 || object.item_script_on_use() == 0 {
            return false;
        }
        engine
            .globals
            .game
            .script_entries
            .get(object.item_script_on_use() as usize)
            .is_some_and(|entry| entry.operation == 0x0081 && entry.operand[0] == event_id)
    })
}

fn metric(a: (i32, i32), b: (i32, i32)) -> i32 {
    (a.0 - b.0).abs() + (a.1 - b.1).abs() * 2
}

fn script_destination_scene(engine: &Engine, script: u16) -> Option<u16> {
    let start = script as usize;
    for index in start..start.saturating_add(16) {
        let entry = engine.globals.game.script_entries.get(index)?;
        if entry.operation == 0x0059 && entry.operand[0] != 0 {
            return Some(entry.operand[0]);
        }
        if entry.operation == 0x0000 {
            break;
        }
    }
    None
}

fn can_search_event_from(position: (i32, i32), event: (i32, i32), mode: u16) -> bool {
    let limit = (mode as usize * 6).saturating_sub(4).min(13);
    for direction in 0..4 {
        let (x_offset, y_offset) = match direction {
            0 => (-16, 8),  // south
            1 => (-16, -8), // west
            2 => (16, -8),  // north
            _ => (16, 8),   // east
        };
        let (mut x, mut y) = position;
        let mut range = [(0i32, 0i32); 13];
        range[0] = position;
        for index in 0..4 {
            range[index * 3 + 1] = (x + x_offset, y + y_offset);
            range[index * 3 + 2] = (x, y + y_offset * 2);
            range[index * 3 + 3] = (x + x_offset * 2, y);
            x += x_offset;
            y += y_offset;
        }
        if range[..limit].contains(&event) {
            return true;
        }
    }
    false
}

fn in_map_bounds(position: (i32, i32)) -> bool {
    (0..8192).contains(&position.0) && (0..4096).contains(&position.1)
}

fn component_anchor(engine: &Engine) -> (i32, i32) {
    let start = player_position(engine);
    let mut anchor = start;
    let mut queue = VecDeque::from([start]);
    let mut seen = HashSet::from([start]);
    while let Some(position) = queue.pop_front() {
        anchor = anchor.min(position);
        if seen.len() > 150_000 {
            break;
        }
        for &(_, delta) in &DIRECTIONS {
            let next = (position.0 + delta.0, position.1 + delta.1);
            if !in_map_bounds(next)
                || seen.contains(&next)
                || engine.check_obstacle_with_range(next, false, 0, true)
            {
                continue;
            }
            seen.insert(next);
            queue.push_back(next);
        }
    }
    anchor
}

fn path_to_event(
    engine: &Engine,
    target: Target,
    known_transition_ids: &HashSet<u16>,
) -> Option<VecDeque<KeyCode>> {
    if let Some(path) = path_to_event_with_collision(engine, target, true, known_transition_ids) {
        return Some(path);
    }
    if known_transition_ids.contains(&target.event_id) {
        return None;
    }
    path_to_event_with_collision(engine, target, false, known_transition_ids)
        .or_else(|| path_to_event_with_collision(engine, target, false, &HashSet::new()))
}

fn path_to_event_with_collision(
    engine: &Engine,
    target: Target,
    check_event_objects: bool,
    known_transition_ids: &HashSet<u16>,
) -> Option<VecDeque<KeyCode>> {
    let start = player_position(engine);
    let event = target.event;
    let goal = event_position(event);
    let trigger_distance = if event.trigger_mode >= 4 {
        ((event.trigger_mode - 4) as i32 * 32 + 16).max(16)
    } else {
        0
    };
    let reached = |position| {
        if event.trigger_mode < 4 {
            can_search_event_from(position, goal, event.trigger_mode.max(1))
        } else {
            metric(position, goal) < trigger_distance
        }
    };
    if reached(start) {
        return Some(VecDeque::new());
    }

    let mut queue = VecDeque::from([start]);
    let mut previous: HashMap<(i32, i32), ((i32, i32), KeyCode)> = HashMap::new();
    let mut seen = HashSet::from([start]);
    let mut found = None;
    let scene = engine.globals.num_scene as usize;
    let foreign_touch_zones = if scene > 0 && scene < engine.globals.game.scenes.len() {
        let first = engine.globals.game.scenes[scene - 1].event_object_index as usize;
        let last = engine.globals.game.scenes[scene].event_object_index as usize;
        (first..last)
            .filter_map(|index| {
                let event_id = (index + 1) as u16;
                let event = engine.globals.game.event_objects[index];
                if event_id == target.event_id
                    || (target.event.trigger_mode >= 4 && metric(event_position(event), goal) < 96)
                    || !known_transition_ids.contains(&event_id)
                    || event.state <= 0
                    || event.vanish_time != 0
                    || event.trigger_script == 0
                    || event.trigger_mode < 4
                {
                    return None;
                }
                // The engine checks touch triggers before/around movement.
                // Keep a half-step halo around the true radius without
                // sealing narrow paths beside closely packed doorways.
                let radius = (event.trigger_mode - 4) as i32 * 32 + 32;
                Some((event_position(event), radius))
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    while let Some(position) = queue.pop_front() {
        if reached(position) {
            found = Some(position);
            break;
        }
        if seen.len() > 200_000 {
            break;
        }
        for &(key, delta) in &DIRECTIONS {
            let next = (position.0 + delta.0, position.1 + delta.1);
            // Learned scene exits fire before the requested movement is
            // processed. Treat every other known exit as a one-way obstacle
            // so a route to an NPC cannot cross a doorway accidentally. When
            // a scene loads inside an exit radius, allow only outward steps.
            let crosses_foreign_trigger = foreign_touch_zones.iter().any(|&(center, radius)| {
                let here = metric(position, center);
                let there = metric(next, center);
                there < radius && (here >= radius || there < here)
            });
            if !in_map_bounds(next)
                || seen.contains(&next)
                || crosses_foreign_trigger
                || engine.check_obstacle_with_range(next, check_event_objects, 0, true)
            {
                continue;
            }
            seen.insert(next);
            previous.insert(next, (position, key));
            queue.push_back(next);
        }
    }

    let mut at = found?;
    let mut reversed = Vec::new();
    while at != start {
        let &(before, key) = previous.get(&at)?;
        reversed.push(key);
        at = before;
    }
    reversed.reverse();
    Some(reversed.into())
}

struct VideoRecorder {
    child: Child,
    input: Rc<RefCell<Option<BufWriter<std::process::ChildStdin>>>>,
    frame_count: Rc<Cell<u64>>,
    output: String,
}

fn start_video_recorder(engine: &mut Engine, output: String) -> VideoRecorder {
    if let Some(parent) = std::path::Path::new(&output).parent() {
        std::fs::create_dir_all(parent).expect("create video output directory");
    }
    let mut child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "warning",
            "-y",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgba",
            "-video_size",
            "320x200",
            "-framerate",
            "30",
            "-i",
            "pipe:0",
            "-an",
            "-vf",
            "scale=960:720:flags=neighbor,pad=1280:720:160:0:black",
            "-c:v",
            "hevc_videotoolbox",
            "-b:v",
            "3M",
            "-realtime",
            "true",
            "-prio_speed",
            "true",
            "-tag:v",
            "hvc1",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            &output,
        ])
        .stdin(Stdio::piped())
        .spawn()
        .expect("start ffmpeg H.265 encoder");
    let input = Rc::new(RefCell::new(Some(BufWriter::new(
        child.stdin.take().expect("ffmpeg stdin"),
    ))));
    let frame_count = Rc::new(Cell::new(0u64));
    let sink_input = Rc::clone(&input);
    let sink_count = Rc::clone(&frame_count);
    engine.frame_sink = Some(Box::new(move |rgba, _ticks| {
        sink_input
            .borrow_mut()
            .as_mut()
            .expect("video input still open")
            .write_all(rgba)
            .expect("write video frame");
        sink_count.set(sink_count.get() + 1);
    }));
    VideoRecorder {
        child,
        input,
        frame_count,
        output,
    }
}

fn finish_video_recorder(engine: &mut Engine, mut recorder: VideoRecorder) {
    engine.frame_sink = None;
    recorder.input.borrow_mut().take();
    let status = recorder.child.wait().expect("wait for ffmpeg encoder");
    assert!(status.success(), "ffmpeg H.265 encoder failed: {status}");
    eprintln!(
        "recorded {} frames to {}",
        recorder.frame_count.get(),
        recorder.output
    );
}

fn main() {
    std::env::set_var("PAL_DATA_DIR", concat!(env!("CARGO_MANIFEST_DIR"), "/pal"));
    // Keep script timing and captured-frame timestamps in game milliseconds
    // while completing an offscreen probe much faster than wall time.
    std::env::set_var("RUSTPAL_HEADLESS_TIME_SCALE", "100");
    let mut engine = Engine::new(true).expect("headless engine");
    engine.init_ui().expect("initialize UI");
    let mut recorder = std::env::var("RUSTPAL_AUTOPLAY_VIDEO")
        .ok()
        .map(|output| start_video_recorder(&mut engine, output));
    engine.globals.in_main_game = true;
    if let Ok(path) = std::env::var("RUSTPAL_AUTOPLAY_RESUME") {
        let bytes = std::fs::read(&path).expect("read autoplay checkpoint");
        engine
            .globals
            .load_game_from_bytes(&bytes)
            .expect("load autoplay checkpoint");
        engine.globals.load_flags = LOAD_SCENE | LOAD_PLAYER_SPRITE;
        eprintln!(
            "resuming {} at scene={} position={:?}",
            path,
            engine.globals.num_scene,
            player_position(&engine)
        );
    } else {
        engine.globals.current_save_slot = 0;
        engine.globals.reload_in_next_tick(0);
    }
    engine.battle_instant = true;
    if let Some(seed) = std::env::var("RUSTPAL_PLAYTHROUGH_SEED")
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
        .filter(|&value| value != 0)
    {
        seed_random(seed);
        eprintln!("PLAYTHROUGH seed={seed}");
    }

    let flags = engine
        .res
        .load_resources(&mut engine.globals)
        .expect("load new game resources");
    if flags.global_data {
        engine.update_equipments();
    }
    engine.input.clear_key_state();
    engine.start_frame();
    eprintln!(
        "PLAYTHROUGH new_game scene={} pos={:?}",
        engine.globals.num_scene,
        player_position(&engine)
    );

    let mut pilot = Pilot::new(&engine);
    let mut iterations = 0u64;
    let mut logged_battles = 0usize;
    while !engine.quit_requested && iterations < MAX_FRAMES {
        // Match Engine::game_main: scene-change scripts set load flags, and
        // the new map/event sprites must be installed before pathfinding or
        // collision checks for the next frame.
        let flags = engine
            .res
            .load_resources(&mut engine.globals)
            .expect("load requested game resources");
        if flags.global_data {
            engine.update_equipments();
        }
        // Script opcode 0x0007 clears auto_battle after every encounter, so
        // arm it again before any trigger can start the next one.
        engine.globals.auto_battle = true;
        arm_party_for_route_probe(&mut engine);
        pilot.prepare_frame(&mut engine);
        if !pilot.skip_start_frame {
            engine.start_frame();
        }
        engine.input.clear_key_state();
        while logged_battles < engine.battle_records.len() {
            let rec = engine.battle_records[logged_battles];
            eprintln!(
                "BATTLE team={} boss={} result={:?} scene={} frame={}",
                rec.enemy_team, rec.is_boss, rec.result, rec.scene, engine.globals.frame_num
            );
            logged_battles += 1;
        }
        iterations += 1;
        if iterations.is_multiple_of(10_000) {
            std::fs::create_dir_all("recordings").expect("create recordings directory");
            std::fs::write(
                "recordings/autoplay-probe-checkpoint.rpg",
                engine.globals.save_game_to_bytes(1),
            )
            .expect("write autoplay checkpoint");
            eprintln!(
                "checkpoint iterations={} scene={} position={:?}",
                iterations,
                engine.globals.num_scene,
                player_position(&engine)
            );
        }
    }
    pilot.release_direction(&mut engine);
    if let Some(recorder) = recorder.take() {
        finish_video_recorder(&mut engine, recorder);
    }

    eprintln!(
        "PLAYTHROUGH complete quit={} iterations={} game_frame={} scene={} pos={:?} battles={}",
        engine.quit_requested,
        iterations,
        engine.globals.frame_num,
        engine.globals.num_scene,
        player_position(&engine),
        engine.battle_records.len()
    );
    println!(
        "autoplay stopped: quit={} iterations={} game_frame={} scene={} position={:?}",
        engine.quit_requested,
        iterations,
        engine.globals.frame_num,
        engine.globals.num_scene,
        player_position(&engine)
    );
    assert!(
        engine.quit_requested,
        "autoplay did not reach the game ending"
    );
}
