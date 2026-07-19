//! `onecad-regen` — a headless regen replay CLI + CI gate.
//!
//! ```text
//! onecad-regen <file.onecad> [--worker <path>] [--json] [--strict]
//! ```
//!
//! Loads a `.onecad` container (onecad-core IO), spawns the **real** OCCT worker
//! (the app's [`WorkerManager`]), replays the whole timeline **from step 0** through
//! the app's [`DocumentRuntime`] regen path, and prints per-op status + the final
//! body signatures + any repair items.
//!
//! ## Exit-code policy (CI gate)
//!
//! * **0** — regen **published** with **zero Failed steps**. A `NeedsRepair` step
//!   alone is *not* a failure: exit 0 with the repairs listed as a warning
//!   (correctness is intact; the model just has unbound refs to resolve).
//!   `--strict` upgrades any `NeedsRepair` to a nonzero exit.
//! * **1** — regen did not publish, a step Failed, or `--strict` + a `NeedsRepair`.
//! * **2** — usage error (bad/missing arguments).
//! * **3** — environment error (no worker binary, worker never became ready, or the
//!   container could not be opened).
//!
//! `--json` emits a single machine-readable object on stdout instead of the human
//! report (diagnostics always go to stderr).

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use onecad_core::document::repair::{RepairItem, RepairReason};
use onecad_core::history::StepState;
use onecad_core::regen::{CancelToken, GeometryEngine, ModelSnapshot, Outcome, RegenRequest};

use onecad_lib::document_runtime::{DocumentRuntime, RegenReport};
use onecad_lib::dto::FeatureStatus;
use onecad_lib::worker::manager::SupervisorConfig;
use onecad_lib::worker::{resolve_worker_path, MeshProvider, SolverEngine, WorkerManager};

const USAGE: &str = "usage: onecad-regen <file.onecad> [--worker <path>] [--json] [--strict]";

/// Parsed command line.
struct Args {
    file: PathBuf,
    worker: Option<PathBuf>,
    json: bool,
    strict: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut file: Option<PathBuf> = None;
    let mut worker: Option<PathBuf> = None;
    let mut json = false;
    let mut strict = false;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--strict" => strict = true,
            "--worker" => {
                let p = it
                    .next()
                    .ok_or_else(|| "--worker requires a path".to_string())?;
                worker = Some(PathBuf::from(p));
            }
            "-h" | "--help" => return Err("help".to_string()),
            other if other.starts_with('-') => {
                return Err(format!("unknown flag {other:?}"));
            }
            other => {
                if file.replace(PathBuf::from(other)).is_some() {
                    return Err("more than one input file given".to_string());
                }
            }
        }
    }
    let file = file.ok_or_else(|| "missing <file.onecad>".to_string())?;
    Ok(Args {
        file,
        worker,
        json,
        strict,
    })
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) if e == "help" => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
        Err(e) => {
            eprintln!("onecad-regen: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(args).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("onecad-regen: {e}");
            ExitCode::from(3)
        }
    }
}

/// Runs the replay. `Err` ⇒ an environment failure (exit 3); `Ok(code)` carries the
/// gate verdict (0 / 1).
async fn run(args: Args) -> Result<ExitCode, String> {
    let worker = args
        .worker
        .clone()
        .or_else(resolve_worker_path)
        .ok_or_else(|| {
            "no worker binary — pass --worker <path> or set ONECAD_WORKER_PATH".to_string()
        })?;
    if !worker.is_file() {
        return Err(format!("worker binary not found at {worker:?}"));
    }

    eprintln!("onecad-regen: spawning worker {worker:?}");
    let wm = WorkerManager::spawn(SupervisorConfig::production(worker));
    if !wm.wait_ready(Duration::from_secs(20)).await {
        return Err("worker never became ready (handshake / OpenSession failed)".into());
    }

    let engine: Arc<dyn GeometryEngine> = Arc::new(wm.clone());
    let meshes: Arc<dyn MeshProvider> = Arc::new(wm.clone());
    let solver: Arc<dyn SolverEngine> = Arc::new(wm.clone());
    let mut rt = DocumentRuntime::open(&args.file, engine, meshes, solver)
        .map_err(|e| format!("open {:?}: {e}", args.file))?;

    eprintln!("onecad-regen: replaying from step 0…");
    let report = rt
        .run_regen(RegenRequest::ToEnd { from: 0 }, CancelToken::new())
        .await;

    let summary = Summary::collect(&rt, &report);
    wm.shutdown().await;

    if args.json {
        println!("{}", summary.to_json());
    } else {
        summary.print_human();
    }

    let exit = summary.exit_code(args.strict);
    Ok(ExitCode::from(exit))
}

/// The gathered replay result: per-op status, final body signatures + the aggregate
/// geometry signature, and any repair items.
struct Summary {
    published: bool,
    outcome: &'static str,
    failed_steps: usize,
    bodies: Vec<(String, String)>,
    geometry_signature: Option<String>,
    ops: Vec<(String, String)>,
    repair: Vec<(usize, String, String)>,
}

impl Summary {
    fn collect(rt: &DocumentRuntime, report: &RegenReport) -> Self {
        let snap: Option<&Arc<ModelSnapshot>> = match &report.outcome {
            Outcome::Published(s) => Some(s),
            _ => None,
        };
        let published = snap.is_some();
        let failed_steps = snap
            .map(|s| {
                s.step_states
                    .iter()
                    .filter(|(_, st)| matches!(st, StepState::Error { .. }))
                    .count()
            })
            .unwrap_or(0);
        let bodies = snap
            .map(|s| {
                s.bodies
                    .iter()
                    .map(|b| (b.body.to_string(), b.signature.as_str().to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let geometry_signature = snap
            .and_then(|s| s.signatures.as_ref())
            .map(|sg| sg.geometry.as_str().to_string());
        let ops = rt
            .projection()
            .features
            .iter()
            .map(|f| (f.label.clone(), feature_status_token(f.status).to_string()))
            .collect();
        let repair = rt
            .repair_items()
            .iter()
            .map(|item| {
                (
                    item.step_index,
                    item.ref_id.clone(),
                    repair_reason_token(item).to_string(),
                )
            })
            .collect();
        Self {
            published,
            outcome: report.outcome_str(),
            failed_steps,
            bodies,
            geometry_signature,
            ops,
            repair,
        }
    }

    /// The CI-gate exit byte (see the module docs).
    fn exit_code(&self, strict: bool) -> u8 {
        if !self.published || self.failed_steps > 0 {
            return 1;
        }
        if strict && !self.repair.is_empty() {
            return 1;
        }
        0
    }

    fn print_human(&self) {
        println!("outcome: {}", self.outcome);
        println!("ops:");
        for (label, status) in &self.ops {
            println!("  - {label}: {status}");
        }
        println!("bodies: {}", self.bodies.len());
        for (id, sig) in &self.bodies {
            println!("  body {id} signature {sig}");
        }
        match &self.geometry_signature {
            Some(sig) => println!("geometry-signature {sig}"),
            None => println!("geometry-signature <none>"),
        }
        if self.repair.is_empty() {
            println!("repair: none");
        } else {
            println!("repair: {} item(s) (NeedsRepair)", self.repair.len());
            for (step, ref_id, reason) in &self.repair {
                println!("  step {step} {ref_id}: {reason}");
            }
        }
        if self.failed_steps > 0 {
            println!("FAILED steps: {}", self.failed_steps);
        }
    }

    fn to_json(&self) -> String {
        let bodies: Vec<_> = self
            .bodies
            .iter()
            .map(|(id, sig)| serde_json::json!({ "bodyId": id, "signature": sig }))
            .collect();
        let ops: Vec<_> = self
            .ops
            .iter()
            .map(|(label, status)| serde_json::json!({ "label": label, "status": status }))
            .collect();
        let repair: Vec<_> = self
            .repair
            .iter()
            .map(|(step, ref_id, reason)| {
                serde_json::json!({ "step": step, "refId": ref_id, "reason": reason })
            })
            .collect();
        serde_json::json!({
            "outcome": self.outcome,
            "published": self.published,
            "failedSteps": self.failed_steps,
            "geometrySignature": self.geometry_signature,
            "bodies": bodies,
            "ops": ops,
            "repair": repair,
        })
        .to_string()
    }
}

fn feature_status_token(s: FeatureStatus) -> &'static str {
    match s {
        FeatureStatus::Ok => "ok",
        FeatureStatus::Dirty => "dirty",
        FeatureStatus::Error => "error",
        FeatureStatus::NeedsRepair => "needsRepair",
    }
}

fn repair_reason_token(item: &RepairItem) -> &'static str {
    match item.reason {
        RepairReason::Ambiguous => "ambiguous",
        RepairReason::NoCandidates => "no-candidates",
        RepairReason::LowConfidence => "low-confidence",
    }
}
