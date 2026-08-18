// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Observability & Metrics Module
//! Provides transparent state tracking and measurement for the Protocol Enforcer

use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};

/// Tracks every state transition with full context
#[derive(Debug, Clone)]
pub struct StateTransition {
    pub timestamp: Instant,
    pub session_id: String,
    pub from_state: String,
    pub to_state: String,
    pub step_id: String,
    pub evidence_count: usize,
    pub missing_items: Vec<String>,
    pub iteration: u32,
}

/// Per-session metrics with full transparency
#[derive(Debug, Clone, Default)]
pub struct SessionMetrics {
    pub session_id: String,
    pub pipeline_id: String,
    pub current_step: Option<String>,
    pub status: String,
    pub total_steps: usize,
    pub completed_steps: usize,
    pub total_attempts: usize,
    pub successful_attempts: usize,
    pub rejected_attempts: usize,
    pub circuit_breaker_triggers: usize,
    pub time_per_step: HashMap<String, Duration>,
    pub current_step_start: Option<Instant>,
    pub session_start: Option<Instant>,
    pub transitions: Vec<StateTransition>,
}

/// Global metrics across all sessions
#[derive(Debug, Clone, Default)]
pub struct GlobalMetrics {
    pub total_sessions: usize,
    pub active_sessions: usize,
    pub completed_sessions: usize,
    pub failed_sessions: usize,
    pub total_steps_completed: usize,
    pub total_rejections: usize,
    pub total_circuit_breakers: usize,
    pub per_step_rejections: HashMap<String, usize>,
    pub per_step_attempts: HashMap<String, usize>,
    pub per_step_avg_time: HashMap<String, Duration>,
}

impl fmt::Display for SessionMetrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let success_rate = if self.total_attempts > 0 {
            (self.successful_attempts as f64 / self.total_attempts as f64) * 100.0
        } else {
            0.0
        };

        let completion_pct = if self.total_steps > 0 {
            (self.completed_steps as f64 / self.total_steps as f64) * 100.0
        } else {
            0.0
        };

        write!(
            f,
            "
╔══════════════════════════════════════════════════════════════╗
║  SESSION METRICS: {:<40} ║
╠══════════════════════════════════════════════════════════════╣
║  Pipeline:        {:<40} ║
║  Current Step:    {:<40} ║
║  Status:          {:<40} ║
║  Progress:        {:>3.0}% ({}/{} steps)                   ║
╠══════════════════════════════════════════════════════════════╣
║  Attempts:        {:<40} ║
║  Successful:      {:<40} ║
║  Rejected:        {:<40} ║
║  Success Rate:    {:>3.0}%                                  ║
║  Circuit Breakers:{:<40} ║
╠══════════════════════════════════════════════════════════════╣
║  TRANSITIONS:                                                ║
",
            self.session_id,
            self.pipeline_id,
            self.current_step.as_deref().unwrap_or("none"),
            self.status,
            completion_pct,
            self.completed_steps,
            self.total_steps,
            self.total_attempts,
            self.successful_attempts,
            self.rejected_attempts,
            success_rate,
            self.circuit_breaker_triggers
        )?;

        for (i, t) in self.transitions.iter().enumerate() {
            writeln!(
                f,
                "║  {}. {} → {} ({}) [{}]  ",
                i + 1,
                t.from_state,
                t.to_state,
                t.step_id,
                if t.missing_items.is_empty() {
                    "OK"
                } else {
                    "REJECTED"
                }
            )?;
        }

        write!(
            f,
            "╚══════════════════════════════════════════════════════════════╝"
        )
    }
}

impl fmt::Display for GlobalMetrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "
╔══════════════════════════════════════════════════════════════╗
║                   GLOBAL METRICS DASHBOARD                  ║
╠══════════════════════════════════════════════════════════════╣
║  Sessions:                                               ║
║    Total:          {:<40} ║
║    Active:         {:<40} ║
║    Completed:      {:<40} ║
║    Failed:         {:<40} ║
╠══════════════════════════════════════════════════════════════╣
║  Execution:                                              ║
║    Steps Completed:{:<40} ║
║    Total Rejections:{:<39} ║
║    Circuit Breakers:{:<39} ║
╠══════════════════════════════════════════════════════════════╣
║  PER-STEP BREAKDOWN:                                     ║
",
            self.total_sessions,
            self.active_sessions,
            self.completed_sessions,
            self.failed_sessions,
            self.total_steps_completed,
            self.total_rejections,
            self.total_circuit_breakers
        )?;

        let mut steps: Vec<_> = self.per_step_attempts.keys().collect();
        steps.sort();

        for step in steps {
            let attempts = self.per_step_attempts.get(step).unwrap_or(&0);
            let rejections = self.per_step_rejections.get(step).unwrap_or(&0);
            let success = if *attempts > 0 {
                ((*attempts - *rejections) as f64 / *attempts as f64) * 100.0
            } else {
                0.0
            };
            writeln!(
                f,
                "║    {:<20} attempts:{:<4} rejections:{:<4} success:{:>5.1}%  ║",
                step, attempts, rejections, success
            )?;
        }

        write!(
            f,
            "╚══════════════════════════════════════════════════════════════╝"
        )
    }
}

/// Observer that records all state transitions
pub struct MetricsObserver {
    pub sessions: HashMap<String, SessionMetrics>,
    pub global: GlobalMetrics,
}

impl Default for MetricsObserver {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsObserver {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            global: GlobalMetrics::default(),
        }
    }

    pub fn record_start(&mut self, session_id: &str, pipeline_id: &str, total_steps: usize) {
        let metrics = SessionMetrics {
            session_id: session_id.to_string(),
            pipeline_id: pipeline_id.to_string(),
            total_steps,
            session_start: Some(Instant::now()),
            status: "Active".to_string(),
            ..Default::default()
        };
        self.sessions.insert(session_id.to_string(), metrics);
        self.global.total_sessions += 1;
        self.global.active_sessions += 1;
    }

    pub fn record_attempt(&mut self, session_id: &str, step_id: &str, _evidence_count: usize) {
        if let Some(m) = self.sessions.get_mut(session_id) {
            m.total_attempts += 1;
            m.current_step_start = Some(Instant::now());
        }
        *self
            .global
            .per_step_attempts
            .entry(step_id.to_string())
            .or_insert(0) += 1;
    }

    pub fn record_success(&mut self, session_id: &str, step_id: &str) {
        if let Some(m) = self.sessions.get_mut(session_id) {
            m.successful_attempts += 1;
            m.completed_steps += 1;
            m.current_step = Some(step_id.to_string());
            if let Some(start) = m.current_step_start {
                m.time_per_step.insert(step_id.to_string(), start.elapsed());
            }
            m.transitions.push(StateTransition {
                timestamp: Instant::now(),
                session_id: session_id.to_string(),
                from_state: "attempting".to_string(),
                to_state: "completed".to_string(),
                step_id: step_id.to_string(),
                evidence_count: 0,
                missing_items: vec![],
                iteration: m.total_attempts as u32,
            });
        }
        self.global.total_steps_completed += 1;
    }

    pub fn record_rejection(&mut self, session_id: &str, step_id: &str, missing: &[String]) {
        if let Some(m) = self.sessions.get_mut(session_id) {
            m.rejected_attempts += 1;
            m.transitions.push(StateTransition {
                timestamp: Instant::now(),
                session_id: session_id.to_string(),
                from_state: "attempting".to_string(),
                to_state: "rejected".to_string(),
                step_id: step_id.to_string(),
                evidence_count: 0,
                missing_items: missing.to_vec(),
                iteration: m.total_attempts as u32,
            });
        }
        self.global.total_rejections += 1;
        *self
            .global
            .per_step_rejections
            .entry(step_id.to_string())
            .or_insert(0) += 1;
    }

    pub fn record_circuit_breaker(&mut self, session_id: &str, step_id: &str) {
        if let Some(m) = self.sessions.get_mut(session_id) {
            m.circuit_breaker_triggers += 1;
            m.status = "Failed".to_string();
            m.transitions.push(StateTransition {
                timestamp: Instant::now(),
                session_id: session_id.to_string(),
                from_state: "attempting".to_string(),
                to_state: "circuit_breaker".to_string(),
                step_id: step_id.to_string(),
                evidence_count: 0,
                missing_items: vec![],
                iteration: m.total_attempts as u32,
            });
        }
        self.global.total_circuit_breakers += 1;
        self.global.failed_sessions += 1;
        self.global.active_sessions = self.global.active_sessions.saturating_sub(1);
    }

    pub fn record_completion(&mut self, session_id: &str) {
        if let Some(m) = self.sessions.get_mut(session_id) {
            m.status = "Completed".to_string();
        }
        self.global.completed_sessions += 1;
        self.global.active_sessions = self.global.active_sessions.saturating_sub(1);
    }

    pub fn print_report(&self) {
        println!("{}", self.global);
        for metrics in self.sessions.values() {
            println!("{}\n", metrics);
        }
    }
}
