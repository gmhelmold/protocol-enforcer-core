// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Step-sizing metrics (SPEC_step_metrics.md). Pure, replay-only analytics
//! computed offline over an already-replayed `Vec<FsmEvent>`. ZERO hot-path
//! overhead: no new events, no new writes on the serving path.

use protocol_types::{FsmEvent, FsmEventType, Profile};
use std::collections::BTreeMap;

/// Per-step (macro/sub position) metrics for a single session.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StepMetrics {
    pub macro_id: String,
    pub sub_state_id: String,
    pub sub_state_type: Option<String>,
    pub criteria_count: Option<usize>,
    pub rejections: usize,
    pub passed: bool,
    pub retry_count: usize,
    pub rejected_criteria: BTreeMap<String, usize>,
    pub dwell_ms: Option<i64>,
}

/// Metrics for a whole session, one `StepMetrics` per distinct `step_id`
/// in first-seen order.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionMetrics {
    pub session_id: String,
    pub pipeline_id: Option<String>,
    pub status: String,
    pub total_rejections: usize,
    pub total_dwell_ms: Option<i64>,
    pub steps: Vec<StepMetrics>,
}

/// One row per distinct checklist criteria_count, aggregated ACROSS sessions.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CriteriaBucket {
    pub criteria_count: usize,
    pub gate_count: usize,
    pub total_rejections: usize,
    pub mean_rejections_per_gate: f64,
    pub first_pass_rate: f64,
    /// Mean dwell (ms) across the gates of this size that recorded a dwell —
    /// the live cost signal: with presence-only checklists a capable agent
    /// rarely rejects, so time-per-gate is what actually grows with gate size.
    /// `None` if no gate of this size recorded a dwell.
    pub mean_dwell_ms: Option<f64>,
}

/// Split a `step_id` of the form `"macro/sub"` on the FIRST `/`.
fn split_step_id(step_id: &str) -> (String, String) {
    match step_id.split_once('/') {
        Some((macro_id, sub_id)) => (macro_id.to_string(), sub_id.to_string()),
        None => (step_id.to_string(), String::new()),
    }
}

/// Look up `sub_state_type`/`criteria_count` for a position in the profile.
fn lookup_profile(
    profile: Option<&Profile>,
    macro_id: &str,
    sub_state_id: &str,
) -> (Option<String>, Option<usize>) {
    let Some(profile) = profile else {
        return (None, None);
    };
    let Some(state) = profile.pipeline.iter().find(|s| s.state_id == macro_id) else {
        return (None, None);
    };
    let Some(sub) = state.sub_states.iter().find(|s| s.id == sub_state_id) else {
        return (None, None);
    };
    let sub_type = Some(sub.sub_state_type.to_string());
    let criteria_count = sub.criteria.as_ref().map(|c| c.len());
    (sub_type, criteria_count)
}

/// A maximal contiguous run of events sharing the same `step_id`. Ordinarily
/// a step_id has exactly one run per session; SPEC_loopback.md's
/// `MacroLoopedBack` makes a macro's execute sub AND its checklist each
/// appear in more than one run when the macro loops (the loop-back event is
/// anchored at the ARRIVAL position -- the execute sub -- so it naturally
/// starts a new run there while ending the checklist's run, exactly like any
/// other step_id-changing transition).
struct Run {
    step_id: String,
    start: usize,
    end: usize,
}

fn is_pass_event(event_type: &FsmEventType) -> bool {
    matches!(
        event_type,
        FsmEventType::MilestoneAccepted { .. } | FsmEventType::SessionCompleted { .. }
    )
}

/// Compute per-session metrics from a session's ordered events. `profile`
/// (optional) supplies `sub_state_type` + `criteria_count`; without it those
/// stay `None`.
///
/// Loop-back attribution (SPEC_loopback.md, real rewrite of the per-step
/// loop, not an added match arm): `passed` and dwell are computed from the
/// session's step_id RUNS, not a single first-occurrence index, so a
/// revisited step (via `MacroLoopedBack`) is attributed across every visit.
/// `passed` is decoupled from the dwell boundary: it is TRUE only if some
/// run for this step_id contains a `MilestoneAccepted`/`SessionCompleted` --
/// a run whose only boundary is a `MacroLoopedBack` (leaving the checklist,
/// arriving at the execute sub) is a dwell boundary but never a pass.
pub fn compute_session_metrics(events: &[FsmEvent], profile: Option<&Profile>) -> SessionMetrics {
    let session_id = events
        .first()
        .map(|e| e.session_id.clone())
        .unwrap_or_default();

    let mut pipeline_id = None;
    let mut completed = false;
    let mut failed = false;

    // Order of first appearance of each distinct step_id.
    let mut order: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for event in events {
        match &event.event_type {
            FsmEventType::SessionStarted {
                pipeline_id: pid, ..
            } => {
                pipeline_id = Some(pid.clone());
            }
            FsmEventType::SessionCompleted { .. } => completed = true,
            FsmEventType::SessionFailed { .. } => failed = true,
            FsmEventType::CircuitBreakerTriggered { .. } => failed = true,
            _ => {}
        }
        if let Some(sid) = &event.step_id {
            if seen.insert(sid.clone()) {
                order.push(sid.clone());
            }
        }
    }

    let status = if completed {
        "completed"
    } else if failed {
        "failed"
    } else {
        "active"
    }
    .to_string();

    // Partition the whole event stream into maximal contiguous runs sharing
    // the same step_id (events with no step_id, e.g. SessionStarted, don't
    // start/extend a run).
    let mut runs: Vec<Run> = Vec::new();
    for (i, event) in events.iter().enumerate() {
        let Some(sid) = &event.step_id else { continue };
        match runs.last_mut() {
            Some(r) if &r.step_id == sid => r.end = i,
            _ => runs.push(Run {
                step_id: sid.clone(),
                start: i,
                end: i,
            }),
        }
    }

    let mut steps = Vec::with_capacity(order.len());
    for step_id in &order {
        let mut rejections = 0usize;
        let mut rejected_criteria: BTreeMap<String, usize> = BTreeMap::new();
        for event in events
            .iter()
            .filter(|e| e.step_id.as_deref() == Some(step_id.as_str()))
        {
            if let FsmEventType::MilestoneRejected { rejected_items, .. } = &event.event_type {
                rejections += 1;
                for item in rejected_items {
                    *rejected_criteria.entry(item.clone()).or_insert(0) += 1;
                }
            }
        }

        // Dwell sums across EVERY visit (run) of this step_id: for each run,
        // find its boundary -- either an in-run pass event (checklist
        // accepted / terminal SessionCompleted, sharing the run's step_id),
        // or, failing that, the very next event in the whole stream (the
        // arrival at wherever the session went next -- a plain
        // SubStateAdvanced/MacroAdvanced, OR a MacroLoopedBack anchored at
        // the loop target). A run with neither is the session's still-active
        // tail: no boundary yet, contributes no dwell.
        let mut passed = false;
        let mut dwell_sum_ms: i64 = 0;
        let mut dwell_found = false;

        for run in runs.iter().filter(|r| &r.step_id == step_id) {
            let pass_idx = (run.start..=run.end).find(|&i| is_pass_event(&events[i].event_type));
            if let Some(pidx) = pass_idx {
                passed = true;
                dwell_sum_ms +=
                    (events[pidx].timestamp - events[run.start].timestamp).num_milliseconds();
                dwell_found = true;
            } else if run.end + 1 < events.len() {
                dwell_sum_ms += (events[run.end + 1].timestamp - events[run.start].timestamp)
                    .num_milliseconds();
                dwell_found = true;
            }
        }

        let dwell_ms = dwell_found.then_some(dwell_sum_ms);

        let (macro_id, sub_state_id) = split_step_id(step_id);
        let (sub_state_type, criteria_count) = lookup_profile(profile, &macro_id, &sub_state_id);

        steps.push(StepMetrics {
            macro_id,
            sub_state_id,
            sub_state_type,
            criteria_count,
            rejections,
            passed,
            retry_count: rejections,
            rejected_criteria,
            dwell_ms,
        });
    }

    let total_rejections = steps.iter().map(|s| s.rejections).sum();
    let total_dwell_ms = match (events.first(), events.last()) {
        (Some(first), Some(last)) => Some((last.timestamp - first.timestamp).num_milliseconds()),
        _ => None,
    };

    SessionMetrics {
        session_id,
        pipeline_id,
        status,
        total_rejections,
        total_dwell_ms,
        steps,
    }
}

/// One row per distinct checklist `criteria_count`, aggregated across
/// sessions — the research payoff: mean rejections per gate vs. gate size.
pub fn aggregate_by_criteria_count(sessions: &[SessionMetrics]) -> Vec<CriteriaBucket> {
    struct Acc {
        gate_count: usize,
        total_rejections: usize,
        first_pass_count: usize,
        dwell_sum_ms: i64,
        dwell_count: usize,
    }

    let mut buckets: BTreeMap<usize, Acc> = BTreeMap::new();
    for session in sessions {
        for step in &session.steps {
            let Some(criteria_count) = step.criteria_count else {
                continue;
            };
            let acc = buckets.entry(criteria_count).or_insert(Acc {
                gate_count: 0,
                total_rejections: 0,
                first_pass_count: 0,
                dwell_sum_ms: 0,
                dwell_count: 0,
            });
            acc.gate_count += 1;
            acc.total_rejections += step.rejections;
            if step.passed && step.rejections == 0 {
                acc.first_pass_count += 1;
            }
            if let Some(d) = step.dwell_ms {
                acc.dwell_sum_ms += d;
                acc.dwell_count += 1;
            }
        }
    }

    buckets
        .into_iter()
        .map(|(criteria_count, acc)| CriteriaBucket {
            criteria_count,
            gate_count: acc.gate_count,
            total_rejections: acc.total_rejections,
            mean_rejections_per_gate: acc.total_rejections as f64 / acc.gate_count as f64,
            first_pass_rate: acc.first_pass_count as f64 / acc.gate_count as f64,
            mean_dwell_ms: (acc.dwell_count > 0)
                .then(|| acc.dwell_sum_ms as f64 / acc.dwell_count as f64),
        })
        .collect()
}
