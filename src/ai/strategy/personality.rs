//! Utility strategy + tactics (pure, testable).
//!
//! Decisioni data-driven: ogni opzione ha scorer 0..1, la personalità pesa
//! le soglie. Aggiungere unità/edifici futuri = nuove righe in tabella +
//! pesi in [`Personality`], mai `if kind == X` nel core.

//! Personalità data-driven + pesi utility (`.ron`, mai branch per-kind).

use crate::units::UnitKind;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Personality {
    pub name: &'static str,
    /// Quante truppe prima di considerare l'attacco.
    pub army_threshold: usize,
    /// 0.0.17 — coraggio: attacca se la win_prob predetta (Lanchester,
    /// vista live + ricordi pesati per età) supera questa soglia.
    pub courage: f32,
    /// Costruisci Engineer prima del secondo Tank (turtle eco).
    pub engineer_first: bool,
    /// Secondo Solar prima di attaccare.
    pub second_solar: bool,
    /// Mix produttivo T1 (kind, peso): la coda insegue queste proporzioni.
    pub mix: [(UnitKind, u32); 3],
    /// Tech reattivi T1 (kind, peso): costruiti SOLO contro comp nemiche che
    /// counterano davvero (edge > soglia, vedi `tech_pick`) — a nemico ignoto
    /// edge 1.0 ovunque e si torna al mix normale. Pesi 0 = mai (baseline).
    /// L'AI scopre così il roster nuovo senza cambiare cervello.
    pub tech_mix: [(UnitKind, u32); 2],
    /// Tetto condiviso tech vivi+accodati (0 = mai, solo main).
    pub max_tech: usize,
    /// Torrette difensive massime (0 = mai, solo turtle).
    pub max_turrets: usize,
    /// Lance turret massime (0 = mai, solo turtle late con eco solida).
    pub max_lance: usize,
    /// 0.0.18 — muri difensivi massimi (0 = mai, solo turtle: schermo
    /// davanti alla prima torretta verso la minaccia).
    pub max_walls: usize,
    /// 0.0.15 — scaling: massimi per tipo (oltre il bootstrap).
    pub max_metals: usize,
    pub max_solars: usize,
    pub max_factories: usize,
    /// 0.0.15 — ingegneri massimi vivi+accodati (code non intasate).
    pub max_engineers: usize,
    /// 0.0.13 — ritirata sotto questa frazione HP (0 = mai).
    pub retreat_hp_frac: f32,
    /// 0.0.17 — focus fire a priorità minaccia quando dominante.
    pub focus_fire: bool,
    /// Tick snapshot (4Hz) oltre il quale si attacca comunque (scripted
    /// baseline). `u64::MAX` = mai: solo potenza/soglie decidono.
    pub attack_at_tick: u64,
    /// 0.0.19 — peso minaccia nella frontiera scout
    /// (`novelty − minaccia_norm×peso` in `scout::frontier_target`).
    /// Turtle cauto (1.2) devia di più, rusher audace (0.5) esplora dritto.
    pub scout_threat_weight: f32,
    /// 0.0.20 — commit del capitale: il Commander marcia nell'ondata solo se
    /// la win_prob predetta supera questa soglia (deterministico: soglia, non
    /// dado). 2.0 = mai (baseline prevedibili). Il capitale non si rischia
    /// per ondate ordinarie: muore lui = game over.
    pub commander_commit_prob: f32,
    /// 0.0.22 — pesi utility full (Graham/Lewis/DA:I): ogni scorer macro
    /// (`build/enqueue/scout/defense` in `utility.rs`) è moltiplicato per il
    /// suo peso e gated a `UTILITY_GATE`. Default 1.0 = comportamento
    /// bit-identico (solo gli zeri veri saltano). Tuning futuro senza toccare
    /// il core.
    pub w_build: f32,
    pub w_enqueue: f32,
    pub w_scout: f32,
    pub w_defense: f32,
}

impl Personality {
    pub const TURTLE: Self = Self {
        name: "turtle",
        army_threshold: 4,
        courage: 0.65,
        engineer_first: true,
        second_solar: true,
        mix: [
            (UnitKind::HeavyTank, 5),
            (UnitKind::LightTank, 2),
            (UnitKind::Artillery, 3),
        ],
        tech_mix: [(UnitKind::MortarTank, 1), (UnitKind::MgTank, 1)],
        max_tech: 2,
        max_turrets: 2,
        max_lance: 1,
        max_walls: 3,
        max_metals: 3,
        max_solars: 3,
        max_factories: 2,
        max_engineers: 2,
        retreat_hp_frac: 0.35,
        focus_fire: true,
        attack_at_tick: u64::MAX,
        scout_threat_weight: 1.2,
        commander_commit_prob: 0.95,
        w_build: 1.0,
        w_enqueue: 1.0,
        w_scout: 1.0,
        w_defense: 1.0,
    };
    pub const RUSHER: Self = Self {
        name: "rusher",
        army_threshold: 2,
        courage: 0.55,
        engineer_first: false,
        second_solar: false,
        mix: [
            (UnitKind::HeavyTank, 6),
            (UnitKind::LightTank, 3),
            (UnitKind::Artillery, 1),
        ],
        tech_mix: [(UnitKind::MgTank, 1), (UnitKind::MortarTank, 1)],
        max_tech: 2,
        max_turrets: 0,
        max_lance: 0,
        max_walls: 0,
        max_metals: 2,
        max_solars: 2,
        max_factories: 1,
        max_engineers: 1,
        retreat_hp_frac: 0.25,
        focus_fire: true,
        attack_at_tick: u64::MAX,
        scout_threat_weight: 0.5,
        commander_commit_prob: 0.9,
        w_build: 1.0,
        w_enqueue: 1.0,
        w_scout: 1.0,
        w_defense: 1.0,
    };
    /// Baseline eco-only per la league: costruisce economia fino ai cap, non
    /// attacca mai (soglia impossibile), nessun micro aggressivo. Sacco da
    /// boxe: perderci è regressione critica.
    pub const ECO_ONLY: Self = Self {
        name: "eco-only",
        army_threshold: usize::MAX,
        courage: 1.1,
        engineer_first: false,
        second_solar: true,
        mix: [
            (UnitKind::Engineer, 2),
            (UnitKind::Scout, 1),
            (UnitKind::HeavyTank, 0),
        ],
        tech_mix: [(UnitKind::Scout, 0), (UnitKind::Scout, 0)],
        max_tech: 0,
        max_turrets: 0,
        max_lance: 0,
        max_walls: 0,
        max_metals: 3,
        max_solars: 3,
        max_factories: 2,
        max_engineers: 3,
        retreat_hp_frac: 0.0,
        focus_fire: false,
        attack_at_tick: u64::MAX,
        scout_threat_weight: 1.0,
        commander_commit_prob: 2.0,
        w_build: 1.0,
        w_enqueue: 1.0,
        w_scout: 1.0,
        w_defense: 1.0,
    };
    /// Baseline rush-scripted per la league: bootstrap + solo HeavyTank,
    /// ondata a tempo fisso comunque vada, niente ritirate né focus.
    /// Pugile prevedibile per misurare la difesa.
    pub const RUSH_SCRIPTED: Self = Self {
        name: "rush-scripted",
        army_threshold: 2,
        courage: 0.55,
        engineer_first: false,
        second_solar: false,
        mix: [
            (UnitKind::HeavyTank, 1),
            (UnitKind::LightTank, 0),
            (UnitKind::Artillery, 0),
        ],
        tech_mix: [(UnitKind::Scout, 0), (UnitKind::Scout, 0)],
        max_tech: 0,
        max_turrets: 0,
        max_lance: 0,
        max_walls: 0,
        max_metals: 2,
        max_solars: 2,
        max_factories: 1,
        max_engineers: 0,
        retreat_hp_frac: 0.0,
        focus_fire: false,
        attack_at_tick: 480, // 120s: l'ondata parte a tempo, comunque vada
        scout_threat_weight: 0.5,
        commander_commit_prob: 2.0,
        w_build: 1.0,
        w_enqueue: 1.0,
        w_scout: 1.0,
        w_defense: 1.0,
    };

    pub fn from_name(name: &str) -> Self {
        Self::try_from_name(name).unwrap_or(Self::TURTLE)
    }

    /// 0.0.21 — strict come `league::resolve_brain`: nomi ignoti sono errore,
    /// mai fallback silenzioso. `from_name()` resta compat (fallback turtle).
    pub fn try_from_name(name: &str) -> Result<Self, String> {
        match name {
            "turtle" => Ok(Self::TURTLE),
            "rusher" => Ok(Self::RUSHER),
            "eco-only" => Ok(Self::ECO_ONLY),
            "rush-scripted" => Ok(Self::RUSH_SCRIPTED),
            other => Err(format!(
                "Unknown personality '{other}'; use one of: turtle, rusher, eco-only, rush-scripted"
            )),
        }
    }

    /// 0.0.21 — carica da `.ron` (`personalities/*.ron`, `ron` transitiva via
    /// bevy, vedi `cargo tree -i ron`). Il `name` viene leakato a `&'static`
    /// (4 caricamenti, mai hot-reload). Errore = stringa, mai panic.
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let def: PersonalityDef = ron::de::from_str(text).map_err(|e| e.to_string())?;
        Ok(Self {
            name: Box::leak(def.name.into_boxed_str()),
            army_threshold: def.army_threshold,
            courage: def.courage,
            engineer_first: def.engineer_first,
            second_solar: def.second_solar,
            mix: def.mix,
            tech_mix: def.tech_mix,
            max_tech: def.max_tech,
            max_turrets: def.max_turrets,
            max_lance: def.max_lance,
            max_walls: def.max_walls,
            max_metals: def.max_metals,
            max_solars: def.max_solars,
            max_factories: def.max_factories,
            max_engineers: def.max_engineers,
            retreat_hp_frac: def.retreat_hp_frac,
            focus_fire: def.focus_fire,
            attack_at_tick: def.attack_at_tick,
            scout_threat_weight: def.scout_threat_weight,
            commander_commit_prob: def.commander_commit_prob,
            w_build: def.w_build,
            w_enqueue: def.w_enqueue,
            w_scout: def.w_scout,
            w_defense: def.w_defense,
        })
    }
}

/// 0.0.21 — forma serializzabile di `Personality` per `.ron` (nome owned,
/// resto identico). `Personality` resta `Copy` con `&'static str` per il sim;
/// la conversione fa `Box::leak` una volta per caricamento.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PersonalityDef {
    pub name: String,
    pub army_threshold: usize,
    pub courage: f32,
    pub engineer_first: bool,
    pub second_solar: bool,
    pub mix: [(UnitKind, u32); 3],
    pub tech_mix: [(UnitKind, u32); 2],
    pub max_tech: usize,
    pub max_turrets: usize,
    pub max_lance: usize,
    pub max_walls: usize,
    pub max_metals: usize,
    pub max_solars: usize,
    pub max_factories: usize,
    pub max_engineers: usize,
    pub retreat_hp_frac: f32,
    pub focus_fire: bool,
    pub attack_at_tick: u64,
    pub scout_threat_weight: f32,
    pub commander_commit_prob: f32,
    pub w_build: f32,
    pub w_enqueue: f32,
    pub w_scout: f32,
    pub w_defense: f32,
}

impl From<&Personality> for PersonalityDef {
    fn from(p: &Personality) -> Self {
        Self {
            name: p.name.to_owned(),
            army_threshold: p.army_threshold,
            courage: p.courage,
            engineer_first: p.engineer_first,
            second_solar: p.second_solar,
            mix: p.mix,
            tech_mix: p.tech_mix,
            max_tech: p.max_tech,
            max_turrets: p.max_turrets,
            max_lance: p.max_lance,
            max_walls: p.max_walls,
            max_metals: p.max_metals,
            max_solars: p.max_solars,
            max_factories: p.max_factories,
            max_engineers: p.max_engineers,
            retreat_hp_frac: p.retreat_hp_frac,
            focus_fire: p.focus_fire,
            attack_at_tick: p.attack_at_tick,
            scout_threat_weight: p.scout_threat_weight,
            commander_commit_prob: p.commander_commit_prob,
            w_build: p.w_build,
            w_enqueue: p.w_enqueue,
            w_scout: p.w_scout,
            w_defense: p.w_defense,
        }
    }
}
