// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Loop-back application, split out of
//! `profile_engine.rs::reject_checklist` to keep that file under the 600
//! LOC cap (`scripts/check_loc.sh`). A second `impl<L: LedgerPort>
//! ProfileFsmEngine<L>` block, same crate, same type -- Rust allows the
//! methods of one type to be spread across modules like this.

use crate::injection::render_loop_target;
use crate::profile_engine::{ProfileFsmEngine, StepOutcome, StepView};
use chrono::Utc;
use protocol_ledger::LedgerPort;
use protocol_types::{FsmError, FsmEventType, Position};

impl<L: LedgerPort> ProfileFsmEngine<L> {
    /// Applies a loop-back for the checklist rejection at `pos` in macro
    /// `macro_idx`: resolves the macro's first enabled Execute sub-state,
    /// appends `MacroLoopedBack` anchored at that ARRIVAL position (not
    /// `pos`, the checklist -- the exception to the sibling `append` calls
    /// in `reject_checklist`, which all use `pos`), repositions the session
    /// there WITHOUT resetting `macro_iteration` /
    /// `consecutive_identical_rejections` / `last_rejected_evidence_keys`
    /// (the bounding invariant -- mirrors `reject_checklist`'s two circuit
    /// breakers, which run before this is ever reached), and returns
    /// `StepOutcome::LoopedBack`. The caller (`reject_checklist`) has
    /// already verified the AND-gate trigger
    /// (`settings.auto_loop_on_checklist_failure && macro.loop_state`).
    pub(crate) fn apply_loop_back(
        &mut self,
        session_id: &str,
        macro_idx: usize,
        pos: &Position,
        missing: Vec<String>,
        macro_iteration: u32,
    ) -> Result<StepOutcome, FsmError> {
        let macro_def = &self.profile.pipeline[macro_idx];
        let macro_name = macro_def.name.clone();
        let (target, injected) = render_loop_target(macro_def, &self.library)?;

        self.append(
            session_id,
            FsmEventType::MacroLoopedBack {
                from_sub: pos.sub_state_id.clone(),
                to_sub: target.sub_state_id.clone(),
                iteration: macro_iteration,
            },
            &target,
            serde_json::json!({}),
        )?;

        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| FsmError::SessionNotFound(session_id.to_string()))?;
        session.position = Some(target.clone());
        session.updated_at = Utc::now();
        let session_state = session.status.clone();

        Ok(StepOutcome::LoopedBack {
            rejected_items: missing,
            view: StepView {
                position: target,
                injected,
                macro_name,
                session_state,
            },
        })
    }
}
