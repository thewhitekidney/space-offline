//! A node-graph runtime: one root graph instance, its subgraphs and their
//! shared blackboard, run the way the client's `nodegraph.common` runs a
//! graph **[V]** (read on build 3707965, decompiled: `graph.py`, `context.py`,
//! `nodes/{base,blackboard,node_graph,utility,validation,action,getter,event}.py`).
//!
//! The client runs some calls on a new tasklet (`@threadutils.threaded`,
//! `uthread2.start_tasklet`): the blackboard's change messages, a
//! `Blackboard Changed` node's handler, a graph's start and stop, an action.
//! Here those become jobs on one FIFO queue, run until it is empty after each
//! outside call, so the order is the order the tasklets would be scheduled
//! in **[I]**: a tasklet runs when the one that made it yields, and none of
//! these yield in between. `Delay` is a timer, fired by [`Runtime::tick`].
//!
//! Server-only node types (`Start Client Node Graph`, `Send Message To Client
//! Graph`, `Stop Client Node Graph`) and server atoms do not exist in the
//! client; what they do is read off the wire of a live server **[V-TQ]**
//! (Tranquility, build 3707965, 17NOV2026: a new pilot's tutorial) and is
//! returned as [`Effect`]s for the caller to deliver.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::Arc;

use super::data::{GraphData, Library, Param};
use super::value::{Value, compare_values, eval_or_keep};

/// Keyword arguments passed along a flow, as the client's `**kwargs`.
pub type Kw = BTreeMap<String, Value>;

/// What the runtime asks the server to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// `OnStartClientNodeGraph(graph_id, instance, {...})` to the character.
    StartClientGraph { graph_id: i64, instance: String },
    /// `OnStopClientNodeGraph(instance)`.
    StopClientGraph { instance: String },
    /// `OnClientNodeGraphMessage(instance, key, value)`.
    ClientMessage {
        instance: String,
        key: String,
        value: Value,
    },
    /// A checkpoint moved (`Set Checkpoint`); persist it.
    Checkpoint { graph_id: i64, checkpoint: i64 },
    /// A server action this runtime does not perform itself, with its
    /// resolved parameters. Its node waits for [`Runtime::resolve`] with
    /// this `ticket`.
    Action { atom: String, args: Kw, ticket: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sub {
    inst: u64,
    node: usize,
}

#[derive(Default)]
struct NState {
    cached: Kw,
    sync: BTreeSet<usize>,
    child: Option<u64>,
    getter: Option<Kw>,
    epoch: u64,
}

struct Inst {
    graph: Arc<GraphData>,
    uuid: String,
    parent: Option<u64>,
    owner: Option<Sub>,
    exclusive: Option<String>,
    active: bool,
    stopped: bool,
    active_nodes: BTreeSet<usize>,
    children: BTreeSet<u64>,
    stop_listeners: Vec<usize>,
    st: HashMap<usize, NState>,
}

enum Job {
    StartGraph(u64),
    StopGraph(u64),
    BlackboardChanged {
        key: String,
        value: Value,
    },
    ValueChanged {
        at: Sub,
        key: String,
        value: Value,
    },
    Action {
        at: Sub,
        epoch: u64,
        atom: String,
        vars: Kw,
        kw: Kw,
    },
}

struct Timer {
    due: f64,
    at: Sub,
    epoch: u64,
    kw: Kw,
}

/// Runaway guard: a graph that loops on itself stops here, not the server.
const MAX_STEPS: usize = 100_000;

pub struct Runtime {
    lib: Arc<Library>,
    pub character_id: i64,
    bb: BTreeMap<String, Value>,
    bb_subs: Vec<(String, Sub)>,
    msg_subs: Vec<(String, Sub)>,
    events: Vec<(String, Sub)>,
    insts: BTreeMap<u64, Inst>,
    root: u64,
    next: u64,
    queue: VecDeque<Job>,
    timers: Vec<Timer>,
    now: f64,
    rng: u64,
    pub checkpoints: BTreeMap<i64, i64>,
    pub effects: Vec<Effect>,
    pub log: Vec<String>,
    /// Actions waiting for the server: ticket -> (node, its epoch, flow).
    parked: BTreeMap<u64, (Sub, u64, Kw)>,
    next_ticket: u64,
    /// What `Get Item Id For Dungeon Object` answers: `(spawn id, dungeon
    /// object id) -> item id`, kept by the server for the dungeons it
    /// spawned for this run.
    pub dungeon_objects: HashMap<(i64, i64), i64>,
    /// What [`Runtime::state`] hands a node of an instance already stopped:
    /// written to and thrown away, so a node that stops its own graph and
    /// then touches its state does not take the server down.
    scratch: NState,
}

impl Runtime {
    /// Start `graph_id` as a root graph with `blackboard` as its first
    /// values. `checkpoints` are the character's saved ones. `now` is in
    /// seconds, on whatever clock [`Runtime::tick`] is later given; `seed`
    /// feeds the instance-id generator.
    pub fn start(
        lib: Arc<Library>,
        graph_id: i64,
        character_id: i64,
        blackboard: Kw,
        checkpoints: BTreeMap<i64, i64>,
        now: f64,
        seed: u64,
    ) -> Option<Runtime> {
        let graph = lib.graph(graph_id)?;
        let mut rt = Runtime {
            lib,
            character_id,
            bb: BTreeMap::new(),
            bb_subs: Vec::new(),
            msg_subs: Vec::new(),
            events: Vec::new(),
            insts: BTreeMap::new(),
            root: 0,
            next: 1,
            queue: VecDeque::new(),
            timers: Vec::new(),
            now,
            rng: seed ^ 0x9E37_79B9_7F4A_7C15 ^ (character_id as u64).rotate_left(17),
            checkpoints,
            effects: Vec::new(),
            log: Vec::new(),
            parked: BTreeMap::new(),
            next_ticket: 1,
            dungeon_objects: HashMap::new(),
            scratch: NState::default(),
        };
        let root = rt.new_inst(graph, None, None, None);
        rt.root = root;
        let uuid = rt.insts[&root].uuid.clone();
        rt.bb_update("root_graph_instance_id", Value::Str(uuid), false);
        for (k, v) in blackboard {
            rt.bb_update(&k, v, false);
        }
        rt.queue.push_back(Job::StartGraph(root));
        rt.run();
        Some(rt)
    }

    /// The root instance's id, which the client names the server graph by.
    pub fn instance_id(&self) -> &str {
        self.insts.get(&self.root).map_or("", |i| i.uuid.as_str())
    }

    pub fn is_running(&self) -> bool {
        self.insts.get(&self.root).is_some_and(|i| i.active)
    }

    pub fn blackboard(&self, key: &str) -> Value {
        self.bb.get(key).cloned().unwrap_or(Value::None)
    }

    /// `process_client_graph_message`: the client sends a message to the
    /// server graph's context.
    pub fn message(&mut self, key: &str, value: Value) {
        self.send_message(key, value);
        self.run();
    }

    /// An outside event for `EventNode`s with this atom (`Ship Boarded`, ...).
    pub fn event(&mut self, atom: &str, kw: Kw) {
        let subs: Vec<Sub> = self
            .events
            .iter()
            .filter(|(a, _)| a == atom)
            .map(|(_, s)| *s)
            .collect();
        for s in subs {
            self.invoke_event(s, kw.clone());
        }
        self.run();
    }

    /// Fire due `Delay`s. Returns whether anything ran.
    pub fn tick(&mut self, now: f64) -> bool {
        self.now = now;
        let mut fired = false;
        loop {
            let Some(i) = self.timers.iter().position(|t| t.due <= now) else {
                break;
            };
            let t = self.timers.remove(i);
            if !self.insts.contains_key(&t.at.inst) || self.state(t.at).epoch != t.epoch {
                continue;
            }
            fired = true;
            self.mark(t.at, false);
            self.out(t.at, "output", t.kw, false);
            self.run();
        }
        fired
    }

    /// The earliest pending `Delay`, if any.
    pub fn next_due(&self) -> Option<f64> {
        self.timers.iter().map(|t| t.due).reduce(f64::min)
    }

    /// Stop everything (the character left, or the graph is being replaced).
    pub fn stop(&mut self) {
        self.stop_inst(self.root);
        self.run();
    }

    // -- the queue ------------------------------------------------------------

    fn run(&mut self) {
        let mut steps = 0;
        while let Some(job) = self.queue.pop_front() {
            steps += 1;
            if steps > MAX_STEPS {
                self.log.push(format!(
                    "node graph: {MAX_STEPS} steps in one run; queue dropped"
                ));
                self.queue.clear();
                break;
            }
            match job {
                Job::StartGraph(id) => self.start_graph(id),
                Job::StopGraph(id) => self.finish_stop(id),
                Job::BlackboardChanged { key, value } => {
                    let subs: Vec<Sub> = self
                        .bb_subs
                        .iter()
                        .filter(|(k, _)| *k == key)
                        .map(|(_, s)| *s)
                        .collect();
                    for s in subs {
                        self.queue.push_back(Job::ValueChanged {
                            at: s,
                            key: key.clone(),
                            value: value.clone(),
                        });
                    }
                }
                Job::ValueChanged { at, key, value } => self.value_changed(at, &key, value),
                Job::Action {
                    at,
                    epoch,
                    atom,
                    vars,
                    kw,
                } => {
                    // An action queued for an instance stopped since (a scene
                    // ending while its last node's action waits) is dropped.
                    if self.insts.contains_key(&at.inst) && self.state(at).epoch == epoch {
                        self.run_action(at, &atom, vars, kw);
                    }
                }
            }
        }
    }

    // -- instances ------------------------------------------------------------

    fn uuid(&mut self) -> String {
        let mut word = || {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            self.rng
        };
        let (a, b) = (word(), word());
        format!(
            "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
            a >> 32,
            (a >> 16) & 0xffff,
            a & 0xfff,
            0x8000 | ((b >> 48) & 0x3fff),
            b & 0xffff_ffff_ffff
        )
    }

    fn new_inst(
        &mut self,
        graph: Arc<GraphData>,
        parent: Option<u64>,
        owner: Option<Sub>,
        exclusive: Option<String>,
    ) -> u64 {
        let id = self.next;
        self.next += 1;
        let uuid = self.uuid();
        self.insts.insert(
            id,
            Inst {
                graph,
                uuid,
                parent,
                owner,
                exclusive,
                active: false,
                stopped: false,
                active_nodes: BTreeSet::new(),
                children: BTreeSet::new(),
                stop_listeners: Vec::new(),
                st: HashMap::new(),
            },
        );
        id
    }

    fn start_graph(&mut self, id: u64) {
        let Some(inst) = self.insts.get_mut(&id) else {
            return;
        };
        if inst.active || inst.stopped {
            return;
        }
        inst.active = true;
        let graph = inst.graph.clone();
        // `start_active_node_ids`: Root and Node Graph Stopped register in
        // their constructors.
        let roots: Vec<usize> = (0..graph.nodes.len())
            .filter(|&n| matches!(graph.nodes[n].ty.as_str(), "Root" | "Node Graph Stopped"))
            .collect();
        for n in roots {
            self.start_node(Sub { inst: id, node: n }, Kw::new(), false);
        }
    }

    /// `start_sub_graph` + `add_sub_graph`: a sibling with the same
    /// exclusive key is stopped first.
    fn start_sub(
        &mut self,
        parent: u64,
        owner: Sub,
        graph_id: i64,
        exclusive: Option<String>,
    ) -> Option<u64> {
        let graph = self.lib.graph(graph_id)?;
        if let Some(key) = &exclusive {
            let siblings: Vec<u64> = self.insts[&parent]
                .children
                .iter()
                .copied()
                .filter(|c| {
                    self.insts
                        .get(c)
                        .is_some_and(|i| i.exclusive.as_ref() == Some(key))
                })
                .collect();
            for s in siblings {
                self.stop_inst(s);
            }
        }
        let child = self.new_inst(graph, Some(parent), Some(owner), exclusive);
        if let Some(p) = self.insts.get_mut(&parent) {
            p.children.insert(child);
        }
        self.queue.push_back(Job::StartGraph(child));
        Some(child)
    }

    /// `stop_node_graph`: inactive now, the rest on the queue.
    fn stop_inst(&mut self, id: u64) {
        let Some(inst) = self.insts.get_mut(&id) else {
            return;
        };
        if inst.stopped {
            return;
        }
        let was_active = inst.active;
        inst.active = false;
        inst.stopped = true;
        if was_active {
            self.queue.push_back(Job::StopGraph(id));
        } else {
            self.finish_stop(id);
        }
    }

    fn finish_stop(&mut self, id: u64) {
        let Some(inst) = self.insts.get(&id) else {
            return;
        };
        let active: Vec<usize> = inst.active_nodes.iter().copied().collect();
        let listeners = inst.stop_listeners.clone();
        let owner = inst.owner;
        let parent = inst.parent;
        for n in active {
            self.stop_node(Sub { inst: id, node: n });
        }
        // `on_stop()`: Node Graph Stopped nodes connected first, then the
        // owning Subgraph node.
        for n in listeners {
            self.out(Sub { inst: id, node: n }, "output", Kw::new(), true);
        }
        if let Some(o) = owner {
            self.subgraph_stopped(o);
        }
        if let Some(p) = parent.and_then(|p| self.insts.get_mut(&p)) {
            p.children.remove(&id);
        }
        self.bb_subs.retain(|(_, s)| s.inst != id);
        self.msg_subs.retain(|(_, s)| s.inst != id);
        self.events.retain(|(_, s)| s.inst != id);
        self.timers.retain(|t| t.at.inst != id);
        if id != self.root {
            self.insts.remove(&id);
        }
    }

    // -- node plumbing ----------------------------------------------------------

    /// A node's state. **An instance already stopped answers a throwaway
    /// state** ([`Runtime::scratch`]) rather than panicking: the scene-2.1 to
    /// 2.2 handoff stopped an instance whose node then read its state, and
    /// the panic poisoned the world lock and dropped the pilot's connection.
    fn state(&mut self, at: Sub) -> &mut NState {
        match self.insts.get_mut(&at.inst) {
            Some(inst) => inst.st.entry(at.node).or_default(),
            None => {
                self.scratch = NState::default();
                &mut self.scratch
            }
        }
    }

    fn mark(&mut self, at: Sub, on: bool) {
        if let Some(i) = self.insts.get_mut(&at.inst) {
            if on {
                i.active_nodes.insert(at.node);
            } else {
                i.active_nodes.remove(&at.node);
            }
        }
    }

    fn is_active(&self, at: Sub) -> bool {
        self.insts
            .get(&at.inst)
            .is_some_and(|i| i.active_nodes.contains(&at.node))
    }

    fn node(&self, at: Sub) -> Option<(Arc<GraphData>, usize)> {
        self.insts.get(&at.inst).map(|i| (i.graph.clone(), at.node))
    }

    /// `_start_connection`.
    fn out(&mut self, at: Sub, port: &str, mut kw: Kw, stopping: bool) {
        let Some(inst) = self.insts.get(&at.inst) else {
            return;
        };
        if !stopping && !inst.active {
            return;
        }
        let targets = inst.graph.nodes[at.node]
            .edges
            .get(port)
            .cloned()
            .unwrap_or_default();
        kw.insert("from_node_id".into(), Value::Int(at.node as i64));
        for t in targets {
            self.start_node(
                Sub {
                    inst: at.inst,
                    node: t,
                },
                kw.clone(),
                stopping,
            );
        }
    }

    /// Run the validators on `port` (a `validation` port); false at the first
    /// that fails.
    fn validators(&mut self, at: Sub, kw: &Kw) -> bool {
        let Some((g, n)) = self.node(at) else {
            return false;
        };
        for t in g.nodes[n]
            .edges
            .get("validation")
            .cloned()
            .unwrap_or_default()
        {
            if self.start_node(
                Sub {
                    inst: at.inst,
                    node: t,
                },
                kw.clone(),
                false,
            ) == Some(false)
            {
                return false;
            }
        }
        true
    }

    /// The node's parameters: the type's (or atom's) defaults, then its own
    /// literals, then whatever its wires carry now
    /// (`_update_variables_from_connections`).
    fn vars(&mut self, at: Sub, depth: usize) -> Kw {
        let Some((g, n)) = self.node(at) else {
            return Kw::new();
        };
        let node = &g.nodes[n];
        let mut out = Kw::new();
        if let Some(t) = self.lib.types.get(&node.ty) {
            out.extend(t.defaults.clone());
        }
        if let Some(a) = node.atom.as_ref().and_then(|a| self.lib.atoms.get(a)) {
            out.extend(a.clone());
        }
        let mut wires: Vec<(String, usize, String)> = Vec::new();
        for (k, p) in node.params.iter().chain(node.atom_params.iter()) {
            match p {
                Param::Lit(v) => {
                    out.insert(k.clone(), v.clone());
                }
                Param::Wire { node, param } => wires.push((k.clone(), *node, param.clone())),
            }
        }
        let mut seen: HashMap<usize, Kw> = HashMap::new();
        for (k, from, param) in wires {
            if depth > 32 {
                out.insert(k, Value::None);
                continue;
            }
            seen.entry(from).or_insert_with(|| {
                self.values(
                    Sub {
                        inst: at.inst,
                        node: from,
                    },
                    depth + 1,
                )
            });
            out.insert(k, seen[&from].get(&param).cloned().unwrap_or(Value::None));
        }
        out
    }

    /// `get_node_values`: what a node offers to the nodes wired to it.
    fn values(&mut self, at: Sub, depth: usize) -> Kw {
        let Some((g, n)) = self.node(at) else {
            return Kw::new();
        };
        let node = &g.nodes[n];
        let vars = self.vars(at, depth);
        let mut out = Kw::new();
        match node.ty.as_str() {
            "Get Blackboard Value" => {
                let v = self.bb_get(&vars);
                let v = if v.is_none() {
                    vars.get("default_value").cloned().unwrap_or(Value::None)
                } else {
                    v
                };
                out.insert("blackboard_value".into(), v);
            }
            "Blackboard Value" | "Blackboard Write" | "Blackboard Changed" => {
                let v = self.bb_get(&vars);
                out.insert("blackboard_value".into(), v);
            }
            "GetterNode" => {
                let cache = vars.get("cache_values").is_some_and(Value::truthy);
                if cache && let Some(c) = &self.state(at).getter {
                    return c.clone();
                }
                out = self.getter(node.atom.as_deref().unwrap_or(""), &vars);
                if cache {
                    self.state(at).getter = Some(out.clone());
                }
            }
            _ => out = self.state(at).cached.clone(),
        }
        out
    }

    fn bb_get(&self, vars: &Kw) -> Value {
        let key = vars
            .get("blackboard_key")
            .map(|k| k.to_string())
            .unwrap_or_default();
        let v = self.bb.get(&key).cloned().unwrap_or(Value::None);
        match vars.get("object_path") {
            Some(Value::Str(path)) if !path.is_empty() => path_get(&v, path),
            _ => v,
        }
    }

    fn bb_update(&mut self, key: &str, value: Value, force: bool) {
        if !force
            && let Some(old) = self.bb.get(key)
            && *old == value
        {
            return;
        }
        self.bb.insert(key.to_string(), value.clone());
        self.queue.push_back(Job::BlackboardChanged {
            key: key.to_string(),
            value,
        });
    }

    fn send_message(&mut self, key: &str, value: Value) {
        let subs: Vec<Sub> = self
            .msg_subs
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, s)| *s)
            .collect();
        for s in subs {
            self.message_received(s, key, value.clone());
        }
    }

    // -- node types -------------------------------------------------------------

    /// `Node.start` for every type. Validation nodes answer Some(valid).
    fn start_node(&mut self, at: Sub, kw: Kw, stopping: bool) -> Option<bool> {
        let (g, n) = self.node(at)?;
        let node = &g.nodes[n];
        let ty = node.ty.as_str();
        // Re-entrant listeners return before reading anything.
        if matches!(
            ty,
            "Node Graph Message Listener" | "Blackboard Changed" | "EventNode" | "Subgraph"
        ) && self.is_active(at)
        {
            return None;
        }
        let vars = self.vars(at, 0);
        let s = |k: &str| vars.get(k).map(|v| v.to_string()).filter(|v| v != "None");
        match ty {
            "Root" => self.out(at, "output", kw, stopping),
            "Node Graph Stopped" => {
                if let Some(i) = self.insts.get_mut(&at.inst)
                    && !i.stop_listeners.contains(&n)
                {
                    i.stop_listeners.push(n);
                }
            }
            "Subgraph" | "Blackboard Triggered Subgraph" => {
                let Some(graph_id) = vars.get("node_graph_id").and_then(Value::as_int) else {
                    self.log.push(format!(
                        "node graph {}: subgraph node without a graph id",
                        g.id
                    ));
                    return None;
                };
                if stopping {
                    // `_start_standalone_graph`: a detached graph on the same blackboard.
                    if let Some(graph) = self.lib.graph(graph_id) {
                        let id = self.new_inst(graph, None, None, None);
                        self.queue.push_back(Job::StartGraph(id));
                    }
                    return None;
                }
                self.mark(at, true);
                match self.start_sub(at.inst, at, graph_id, s("exclusive_key")) {
                    Some(child) => {
                        self.state(at).child = Some(child);
                        self.out(at, "on_start", kw, false);
                    }
                    None => {
                        self.log
                            .push(format!("node graph {}: no graph {graph_id}", g.id));
                        self.mark(at, false);
                    }
                }
            }
            "Stop Node Graph" => self.stop_inst(at.inst),
            "Stop Sub Graph" => {
                if let Some(key) = s("exclusive_key") {
                    let kids: Vec<u64> = self.insts[&at.inst]
                        .children
                        .iter()
                        .copied()
                        .filter(|c| {
                            self.insts
                                .get(c)
                                .is_some_and(|i| i.exclusive.as_deref() == Some(key.as_str()))
                        })
                        .collect();
                    for k in kids {
                        self.stop_inst(k);
                    }
                }
                self.out(at, "output", kw, stopping);
            }
            "Stop Active Nodes" => {
                let active: Vec<usize> =
                    self.insts[&at.inst].active_nodes.iter().copied().collect();
                for a in active {
                    self.stop_node(Sub {
                        inst: at.inst,
                        node: a,
                    });
                }
                self.out(at, "output", kw, stopping);
            }
            "Node Graph Message Listener" => {
                self.mark(at, true);
                if let Some(key) = s("message_key") {
                    self.msg_subs.push((key, at));
                }
            }
            "Send Message To Node Graph" => {
                let key = s("message_key").unwrap_or_default();
                let value = vars.get("message_value").cloned().unwrap_or(Value::None);
                self.send_message(&key, value);
                self.out(at, "output", kw, stopping);
            }
            "Blackboard Changed" => {
                self.mark(at, true);
                let key = s("blackboard_key").unwrap_or_default();
                self.bb_subs.push((key.clone(), at));
                if vars.get("validate_on_start").is_some_and(Value::truthy) {
                    let value = self.bb.get(&key).cloned().unwrap_or(Value::None);
                    self.queue.push_back(Job::ValueChanged { at, key, value });
                }
            }
            "Blackboard Value" => {
                let have = self.bb_get(&vars);
                let want = vars.get("blackboard_value").cloned().unwrap_or(Value::None);
                let op = s("operator");
                let flipped = vars.get("flipped").is_some_and(Value::truthy);
                let valid = compare_values(&have, &want, op.as_deref(), flipped);
                self.out(
                    at,
                    if valid { "on_success" } else { "on_failure" },
                    kw,
                    stopping,
                );
                return Some(valid);
            }
            "Blackboard Write" => {
                let key = s("blackboard_key")?;
                let value = vars
                    .get("blackboard_value")
                    .map(eval_or_keep)
                    .unwrap_or(Value::None);
                let force = vars.get("force_scatter").is_some_and(Value::truthy);
                self.bb_update(&key, value, force);
                self.out(at, "output", kw, stopping);
            }
            "Increment Blackboard Value" | "Decrement Blackboard Value" => {
                if let Some(key) = s("blackboard_key") {
                    let d = if ty.starts_with("Increment") { 1 } else { -1 };
                    let base = self.bb.get(&key).and_then(Value::as_int).unwrap_or(0);
                    self.bb_update(&key, Value::Int(base + d), true);
                }
                self.out(at, "output", kw, stopping);
            }
            "Delay" => {
                let delay = vars.get("delay").and_then(Value::as_float).unwrap_or(1.0);
                let st = self.state(at);
                st.epoch += 1;
                let epoch = st.epoch;
                self.mark(at, true);
                self.timers.retain(|t| t.at != at);
                self.timers.push(Timer {
                    due: self.now + delay,
                    at,
                    epoch,
                    kw,
                });
            }
            "Sync" => {
                if let Some(from) = kw.get("from_node_id").and_then(Value::as_int) {
                    self.state(at).sync.insert(from as usize);
                }
                let inputs: BTreeSet<usize> = g.inputs[n].iter().copied().collect();
                if self.state(at).sync == inputs {
                    self.out(at, "output", kw, stopping);
                }
            }
            "ValidationNode" => {
                let atom = node.atom.as_deref().unwrap_or("");
                let mut valid = validate(atom, &vars, &kw);
                if vars.get("is_not").is_some_and(Value::truthy) {
                    valid = !valid;
                }
                self.out(
                    at,
                    if valid { "on_success" } else { "on_failure" },
                    kw,
                    stopping,
                );
                return Some(valid);
            }
            "Condition Group" | "Validation Group" => {
                let require_all = vars.get("require_all").is_some_and(Value::truthy);
                let run_all = vars.get("run_all").is_some_and(Value::truthy);
                let mut any = false;
                let mut all = true;
                for t in node.edges.get("validation").cloned().unwrap_or_default() {
                    let ok = self.start_node(
                        Sub {
                            inst: at.inst,
                            node: t,
                        },
                        kw.clone(),
                        false,
                    ) == Some(true);
                    any |= ok;
                    all &= ok;
                    if !run_all && ((require_all && !ok) || (!require_all && ok)) {
                        break;
                    }
                }
                let valid = if require_all { all } else { any };
                self.out(
                    at,
                    if valid { "on_success" } else { "on_failure" },
                    kw,
                    stopping,
                );
                return Some(valid);
            }
            "ActionNode" => {
                let st = self.state(at);
                st.epoch += 1;
                let epoch = st.epoch;
                let atom = node.atom.clone().unwrap_or_default();
                self.queue.push_back(Job::Action {
                    at,
                    epoch,
                    atom,
                    vars,
                    kw,
                });
            }
            "EventNode" => {
                self.mark(at, true);
                self.events
                    .push((node.atom.clone().unwrap_or_default(), at));
                if vars.get("validate_on_start").is_some_and(Value::truthy) {
                    self.invoke_event(at, Kw::new());
                }
            }
            "Start Client Node Graph" => {
                let graph_id = vars
                    .get("node_graph_id")
                    .and_then(Value::as_int)
                    .unwrap_or(0);
                let instance = self.uuid();
                let key = s("client_graph_instance_id_blackboard_key")
                    .unwrap_or_else(|| "client_graph_instance_id".into());
                self.bb_update(&key, Value::Str(instance.clone()), false);
                self.state(at).cached = Kw::from([(
                    "client_graph_instance_id".to_string(),
                    Value::Str(instance.clone()),
                )]);
                self.effects
                    .push(Effect::StartClientGraph { graph_id, instance });
                self.out(at, "output", kw, stopping);
            }
            "Send Message To Client Graph" => {
                if let Some(instance) = self.client_instance(&vars) {
                    let key = s("message_key").unwrap_or_default();
                    let value = vars.get("message_value").cloned().unwrap_or(Value::None);
                    self.effects.push(Effect::ClientMessage {
                        instance,
                        key,
                        value,
                    });
                }
                self.out(at, "output", kw, stopping);
            }
            "Stop Client Node Graph" => {
                if let Some(instance) = self.client_instance(&vars) {
                    self.effects.push(Effect::StopClientGraph { instance });
                }
                self.out(at, "output", kw, stopping);
            }
            // Getters answer when asked; starting one does nothing.
            "GetterNode" | "Get Blackboard Value" => {}
            // Analytics and anything else with one output: pass the flow on.
            _ => self.out(at, "output", kw, stopping),
        }
        None
    }

    fn client_instance(&self, vars: &Kw) -> Option<String> {
        if let Some(Value::Str(i)) = vars.get("client_graph_instance_id") {
            return Some(i.clone());
        }
        let key = vars
            .get("client_graph_instance_id_blackboard_key")
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "client_graph_instance_id".into());
        match self.bb.get(&key) {
            Some(Value::Str(i)) => Some(i.clone()),
            _ => None,
        }
    }

    fn stop_node(&mut self, at: Sub) {
        let Some((g, n)) = self.node(at) else { return };
        match g.nodes[n].ty.as_str() {
            "Subgraph" | "Blackboard Triggered Subgraph" => {
                if !self.is_active(at) {
                    return;
                }
                if let Some(child) = self.state(at).child {
                    self.stop_inst(child);
                }
            }
            "Node Graph Message Listener" => {
                self.msg_subs.retain(|(_, s)| *s != at);
                self.mark(at, false);
            }
            "Blackboard Changed" => {
                self.bb_subs.retain(|(_, s)| *s != at);
                self.mark(at, false);
            }
            "EventNode" => {
                self.events.retain(|(_, s)| *s != at);
                self.mark(at, false);
            }
            "Delay" | "ActionNode" => {
                self.state(at).epoch += 1;
                self.timers.retain(|t| t.at != at);
                self.mark(at, false);
            }
            "Sync" => self.state(at).sync.clear(),
            _ => {}
        }
    }

    /// `SubGraphNode._on_stop`.
    fn subgraph_stopped(&mut self, at: Sub) {
        if !self.insts.contains_key(&at.inst) {
            return;
        }
        self.mark(at, false);
        self.state(at).child = None;
        self.out(at, "on_stop", Kw::new(), false);
    }

    /// `NodeGraphMessageListener._message_received`.
    fn message_received(&mut self, at: Sub, key: &str, value: Value) {
        let n = at.node;
        let mut kw = Kw::new();
        kw.insert("from_node_id".into(), Value::Int(n as i64));
        kw.insert(key.to_string(), value.clone());
        kw.insert("value".into(), value.clone());
        let mut cached = Kw::new();
        cached.insert("key".into(), Value::str(key));
        cached.insert("value".into(), value.clone());
        cached.insert(key.to_string(), value);
        self.state(at).cached = cached;
        if !self.validators(at, &kw) {
            return;
        }
        let keep = self
            .vars(at, 0)
            .get("keep_listening")
            .is_some_and(Value::truthy);
        if !keep {
            self.stop_node(at);
        }
        self.out(at, "output", kw, false);
    }

    /// `BlackboardEventNode._value_changed`.
    fn value_changed(&mut self, at: Sub, key: &str, value: Value) {
        if !self.is_active(at) {
            return;
        }
        let mut kw = Kw::new();
        kw.insert("from_node_id".into(), Value::Int(at.node as i64));
        kw.insert(key.to_string(), value);
        if !self.validators(at, &kw) {
            return;
        }
        let keep = self
            .vars(at, 0)
            .get("keep_listening")
            .is_some_and(Value::truthy);
        if !keep {
            self.stop_node(at);
        }
        self.out(at, "output", kw, false);
    }

    /// `EventNode.invoke`.
    fn invoke_event(&mut self, at: Sub, mut kw: Kw) {
        if !self.is_active(at) {
            return;
        }
        kw.insert("from_node_id".into(), Value::Int(at.node as i64));
        self.state(at).cached = kw.clone();
        if !self.validators(at, &kw) {
            return;
        }
        let keep = self
            .vars(at, 0)
            .get("keep_listening")
            .is_some_and(Value::truthy);
        if !keep {
            self.stop_node(at);
        }
        self.out(at, "output", kw, false);
    }

    // -- atoms ------------------------------------------------------------------

    fn getter(&self, atom: &str, vars: &Kw) -> Kw {
        let mut out = Kw::new();
        match atom {
            "Get Checkpoint" => {
                let graph = vars
                    .get("node_graph_id")
                    .and_then(Value::as_int)
                    .unwrap_or(0);
                let v = self
                    .checkpoints
                    .get(&graph)
                    .map_or(Value::None, |c| Value::Int(*c));
                out.insert("checkpoint_id".into(), v);
            }
            "Integer" => {
                out.insert(
                    "value".into(),
                    vars.get("value")
                        .and_then(Value::as_int)
                        .map_or(Value::None, Value::Int),
                );
            }
            "Float" => {
                out.insert(
                    "value".into(),
                    vars.get("value")
                        .and_then(Value::as_float)
                        .map_or(Value::None, Value::Float),
                );
            }
            "Boolean" => {
                out.insert(
                    "value".into(),
                    Value::Bool(vars.get("value").is_some_and(Value::truthy)),
                );
            }
            "String" => {
                out.insert(
                    "value".into(),
                    Value::Str(vars.get("value").map(|v| v.to_string()).unwrap_or_default()),
                );
            }
            "None" => {
                out.insert("value".into(), Value::None);
            }
            "Get Item Id For Dungeon Object" => {
                let spawn = vars.get("dungeon_spawn_id").and_then(Value::as_int);
                let object = vars.get("dungeon_object_id").and_then(Value::as_int);
                let id = spawn
                    .zip(object)
                    .and_then(|k| self.dungeon_objects.get(&k).copied());
                out.insert("item_id".into(), id.map_or(Value::None, Value::Int));
            }
            _ => {}
        }
        out
    }

    fn run_action(&mut self, at: Sub, atom: &str, vars: Kw, mut kw: Kw) {
        let track = vars.get("track_action").is_some_and(Value::truthy);
        if track {
            self.mark(at, true);
        }
        match atom {
            "Set Checkpoint" => {
                let graph_id = vars
                    .get("node_graph_id")
                    .and_then(Value::as_int)
                    .unwrap_or(0);
                if let Some(checkpoint) = vars.get("checkpoint_id").and_then(Value::as_int) {
                    self.checkpoints.insert(graph_id, checkpoint);
                    self.effects.push(Effect::Checkpoint {
                        graph_id,
                        checkpoint,
                    });
                }
            }
            // A server atom: the flow waits here until the server has done
            // it ([`Runtime::resolve`]), so its outputs (`spawn_id`, ...)
            // are there for the nodes wired to it, as the client's atom job
            // returns them before the node's output fires.
            _ => {
                let mut args = vars;
                args.retain(|k, _| k != "track_action");
                kw.remove("from_node_id");
                let ticket = self.next_ticket;
                self.next_ticket += 1;
                let epoch = self.state(at).epoch;
                self.parked.insert(ticket, (at, epoch, kw));
                self.effects.push(Effect::Action {
                    atom: atom.to_string(),
                    args,
                    ticket,
                });
                return;
            }
        }
        kw.remove("from_node_id");
        self.state(at).cached = kw.clone();
        self.out(at, "output", kw, false);
    }

    /// The server did the action of `ticket` (an [`Effect::Action`]): its
    /// `outputs` join what the node offers, and the flow goes on. A ticket
    /// whose node was stopped since, or that is unknown, does nothing.
    pub fn resolve(&mut self, ticket: u64, outputs: Kw) {
        let Some((at, epoch, kw)) = self.parked.remove(&ticket) else {
            return;
        };
        if !self.insts.contains_key(&at.inst) || self.state(at).epoch != epoch {
            return;
        }
        let mut cached = kw.clone();
        cached.extend(outputs);
        self.state(at).cached = cached;
        self.out(at, "output", kw, false);
        self.run();
    }
}

/// `get_object_value_by_path`, for dict-less values: list indexes only.
fn path_get(v: &Value, path: &str) -> Value {
    let mut cur = v.clone();
    for part in path.split('.') {
        cur = match (&cur, part.parse::<usize>()) {
            (Value::List(items), Ok(i)) => items.get(i).cloned().unwrap_or(Value::None),
            _ => return Value::None,
        };
    }
    cur
}

/// The condition atoms the server graphs use.
fn validate(atom: &str, vars: &Kw, kw: &Kw) -> bool {
    let op = vars
        .get("operator")
        .and_then(|v| v.as_str().map(str::to_string));
    let flipped = vars.get("flipped").is_some_and(Value::truthy);
    let get = |k: &str| vars.get(k).cloned().unwrap_or(Value::None);
    match atom {
        "Compare Values" => {
            compare_values(&get("value_a"), &get("value_b"), op.as_deref(), flipped)
        }
        "Match Parameter" => {
            let key = get("parameter_key").to_string();
            match kw.get(&key) {
                Some(v) => compare_values(v, &get("parameter_value"), op.as_deref(), flipped),
                None => false,
            }
        }
        // `literal_eval(str(value)) is None`.
        "Is None" => match get("value") {
            Value::None => true,
            Value::Str(s) => s == "None",
            _ => false,
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented graphs in the bundle format, shaped like a tutorial master:
    // graph 1 starts client graph 9, runs scene graph 2 as an exclusive
    // subgraph, writes every `done` message to `last`, and stops when the
    // scene stops. Scene 2 sends task `t1`, and on `last == t1` sets
    // checkpoint 5 and sends `t2`; on `last == t2` it stops.
    const TYPES: &str = "#type_id\tname\tin_ports\tout_ports\tdefaults\toutputs\n\
        1\tRoot\t\toutput\t\t\n\
        2\tSubgraph\tinput\ton_start,on_stop\t\t\n\
        3\tStart Client Node Graph\tinput\toutput\tclient_graph_instance_id_blackboard_key=s:client_graph_instance_id\t\n\
        4\tSend Message To Client Graph\tinput\toutput\tclient_graph_instance_id_blackboard_key=s:client_graph_instance_id\t\n\
        5\tStop Node Graph\tinput\t\t\t\n\
        6\tNode Graph Message Listener\tinput\toutput,validation\t\t\n\
        7\tBlackboard Write\tinput\toutput\t\t\n\
        8\tBlackboard Changed\tinput\toutput,validation\t\t\n\
        9\tBlackboard Value\tinput\ton_success,on_failure\t\t\n\
        10\tActionNode\tinput\toutput\t\t\n\
        11\tDelay\tinput\toutput\tdelay=f:1.0\t\n\
        12\tGet Blackboard Value\t\t\t\tblackboard_value\n";
    const ATOMS: &str = "#atom_id\tname\tatom_type\tdefaults\toutputs\n1\tSet Checkpoint\t1\t\t\n2\tGrant Thing\t1\t\t\n";
    const GRAPHS: &str = "#graph_id\tname\ttags\tparent\n1\tmaster\tserver\t-\n2\tscene\tserver\t1\n9\tclient\tclient\t-\n";
    const NODES: &str = "#graph_id\tnode\ttype\tatom\tparams\tatom_params\tedges\n\
        1\tr\tRoot\t-\t\t\toutput:sc\n\
        1\tsc\tStart Client Node Graph\t-\tnode_graph_id=i:9\t\toutput:sub\n\
        1\tsub\tSubgraph\t-\tnode_graph_id=i:2;exclusive_key=s:x\t\ton_start:scene|on_stop:end\n\
        1\tscene\tSend Message To Client Graph\t-\tmessage_key=s:start_scene;message_value=s:A\t\t\n\
        1\tend\tStop Node Graph\t-\t\t\t\n\
        1\tr2\tRoot\t-\t\t\toutput:ml\n\
        1\tml\tNode Graph Message Listener\t-\tmessage_key=s:done;keep_listening=i:1\t\toutput:w\n\
        1\tw\tBlackboard Write\t-\tblackboard_key=s:last;force_scatter=i:1;blackboard_value=c:ml.value\t\t\n\
        2\tr\tRoot\t-\t\t\toutput:ch\n\
        2\tch\tBlackboard Changed\t-\tblackboard_key=s:last;keep_listening=i:1;validate_on_start=i:1\t\toutput:v1\n\
        2\tv1\tBlackboard Value\t-\tblackboard_key=s:last;blackboard_value=s:t1\t\ton_success:cp|on_failure:v2\n\
        2\tv2\tBlackboard Value\t-\tblackboard_key=s:last;blackboard_value=s:t2\t\ton_success:stop|on_failure:t1\n\
        2\tt1\tSend Message To Client Graph\t-\tmessage_key=s:start_task;message_value=s:t1\t\t\n\
        2\tcp\tActionNode\tSet Checkpoint\t\tcheckpoint_id=i:5;node_graph_id=i:1\toutput:grant\n\
        2\tgrant\tActionNode\tGrant Thing\t\twho=c:who.blackboard_value\toutput:t2,gw\n\
        2\tgw\tBlackboard Write\t-\tblackboard_key=s:gift;blackboard_value=c:grant.gift\t\t\n\
        2\twho\tGet Blackboard Value\t-\tblackboard_key=s:character_id\t\t\n\
        2\tt2\tDelay\t-\tdelay=f:2.0\t\toutput:t2m\n\
        2\tt2m\tSend Message To Client Graph\t-\tmessage_key=s:start_task;message_value=s:t2\t\t\n\
        2\tstop\tStop Node Graph\t-\t\t\t\n";

    fn lib() -> Arc<Library> {
        Arc::new(Library::parse(GRAPHS, NODES, TYPES, ATOMS).unwrap())
    }

    fn client_messages(rt: &mut Runtime) -> Vec<(String, String)> {
        rt.effects
            .drain(..)
            .filter_map(|e| match e {
                Effect::ClientMessage { key, value, .. } => Some((key, value.to_string())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_scene_runs_its_task_chain_from_client_messages() {
        let bb = Kw::from([("character_id".to_string(), Value::Int(77))]);
        let mut rt = Runtime::start(lib(), 1, 77, bb, BTreeMap::new(), 0.0, 1).unwrap();
        let first = rt.effects.clone();
        assert!(
            matches!(first[0], Effect::StartClientGraph { graph_id: 9, .. }),
            "{first:?}"
        );
        let Effect::StartClientGraph { instance, .. } = &first[0] else {
            unreachable!()
        };
        assert_eq!(
            rt.blackboard("client_graph_instance_id"),
            Value::Str(instance.clone())
        );
        // The scene message goes before the scene's own first task.
        assert_eq!(
            client_messages(&mut rt),
            vec![
                ("start_scene".into(), "A".into()),
                ("start_task".into(), "t1".into())
            ]
        );

        rt.message("done", Value::str("t1"));
        assert_eq!(rt.checkpoints.get(&1), Some(&5));
        let fx = rt.effects.clone();
        assert!(
            fx.contains(&Effect::Checkpoint {
                graph_id: 1,
                checkpoint: 5
            }),
            "{fx:?}"
        );
        let ticket = fx
            .iter()
            .find_map(|e| match e {
                Effect::Action { atom, args, ticket }
                    if atom == "Grant Thing" && args.get("who") == Some(&Value::Int(77)) =>
                {
                    Some(*ticket)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{fx:?}"));
        assert_eq!(rt.next_due(), None, "the flow waits for the server");
        rt.resolve(ticket, Kw::from([("gift".to_string(), Value::Int(7))]));
        assert_eq!(
            rt.blackboard("gift"),
            Value::Int(7),
            "an output reaches a wired node"
        );
        rt.resolve(ticket, Kw::new());
        assert!(
            client_messages(&mut rt).is_empty(),
            "t2 waits for the delay"
        );
        assert_eq!(rt.next_due(), Some(2.0));
        assert!(!rt.tick(1.0));
        assert!(rt.tick(2.0));
        assert_eq!(
            client_messages(&mut rt),
            vec![("start_task".into(), "t2".into())]
        );

        assert!(rt.is_running());
        rt.message("done", Value::str("t2"));
        assert!(!rt.is_running(), "the scene stopped, so the master stopped");
    }

    /// A node of an instance already stopped -- a queued action, a timer, or
    /// a direct read of its state -- is dropped, not a panic (the scene-2.1
    /// to 2.2 handoff took the server down on `expect("live instance")`).
    #[test]
    fn a_stopped_instances_node_is_dropped_not_a_panic() {
        let mut rt = Runtime::start(lib(), 1, 77, Kw::new(), BTreeMap::new(), 0.0, 5).unwrap();
        let gone = Sub {
            inst: 999_999,
            node: 0,
        };
        assert!(!rt.insts.contains_key(&gone.inst));
        rt.state(gone).epoch += 1;
        assert_eq!(rt.state(gone).epoch, 0, "a throwaway state");
        rt.queue.push_back(Job::Action {
            at: gone,
            epoch: 0,
            atom: "Grant Thing".into(),
            vars: Kw::new(),
            kw: Kw::new(),
        });
        rt.timers.push(Timer {
            at: gone,
            epoch: 0,
            due: 0.5,
            kw: Kw::new(),
        });
        rt.effects.clear();
        rt.run();
        assert!(!rt.tick(1.0), "the dead timer does not fire");
        assert!(
            !rt.effects
                .iter()
                .any(|e| matches!(e, Effect::Action { atom, .. } if atom == "Grant Thing")),
            "the dead action does not run"
        );
    }

    #[test]
    fn a_dungeon_objects_item_id_is_what_the_server_recorded() {
        let mut rt = Runtime::start(lib(), 1, 77, Kw::new(), BTreeMap::new(), 0.0, 3).unwrap();
        rt.dungeon_objects.insert((5, 990_001), 42);
        let ask = |spawn: i64| {
            Kw::from([
                ("dungeon_spawn_id".to_string(), Value::Int(spawn)),
                ("dungeon_object_id".to_string(), Value::Int(990_001)),
            ])
        };
        let got = rt.getter("Get Item Id For Dungeon Object", &ask(5));
        assert_eq!(got.get("item_id"), Some(&Value::Int(42)));
        let none = rt.getter("Get Item Id For Dungeon Object", &ask(6));
        assert_eq!(none.get("item_id"), Some(&Value::None));
    }

    #[test]
    fn a_saved_checkpoint_is_what_get_checkpoint_reads() {
        let rt =
            Runtime::start(lib(), 1, 77, Kw::new(), BTreeMap::from([(1, 44)]), 0.0, 2).unwrap();
        assert_eq!(rt.checkpoints.get(&1), Some(&44));
        assert_eq!(rt.instance_id().len(), 36);
    }
}
