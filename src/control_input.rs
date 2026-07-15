//! Mapped MIDI/keyboard/gamepad input for the player, over `vidiotic-ctl`.
//!
//! `vidiotic-ctl` defines its own `Action` vocabulary rather than
//! [`Command`] (it must not depend on this crate — this crate depends on it
//! to embed a `ControlMap` in `.viproj`, so the reverse would cycle); this
//! module owns the `Action -> Command` translation instead.

use crossbeam_channel::Sender;
use vidiotic_ctl::{Action, ControlEvent, ControlMap, ControlSource, EventValue, MidiHub, Mapper, PadPoller};
use winit::keyboard::Key;

use crate::commands::Command;

/// Canonicalize a winit logical key into the same string space
/// `vidiotic_ctl::keys::canon` produces for egui-based capturers (prep, the
/// ctl bin) — single characters lowercase, named keys pass through
/// (`Space`, `ArrowLeft`, `F1`, …; both crates' key enums derive `Debug`
/// following the W3C `KeyboardEvent.key` names, so the strings agree
/// without either side depending on the other). `None` for dead/compose
/// keys, which have no stable identity to bind.
#[must_use]
pub fn canon_key(key: &Key) -> Option<String> {
    match key {
        Key::Character(c) => Some(vidiotic_ctl::keys::canon(c.as_str())),
        Key::Named(named) => Some(vidiotic_ctl::keys::canon(&format!("{named:?}"))),
        _ => None,
    }
}

const RESCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

pub struct ControlInput {
    hub: MidiHub,
    pads: PadPoller,
    mapper: Mapper,
    tx: Sender<ControlEvent>,
    rx: crossbeam_channel::Receiver<ControlEvent>,
    last_rescan: std::time::Instant,
}

impl ControlInput {
    /// `project_map` is this session's `.viproj`-embedded layer; the global
    /// layer loads from the user config dir.
    #[must_use]
    pub fn new(project_map: ControlMap) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let global = vidiotic_ctl::store::load_global();
        Self {
            hub: MidiHub::new(tx.clone()),
            pads: PadPoller::new(),
            mapper: Mapper::new(global, project_map),
            tx,
            rx,
            // Elapsed already past the interval so the first pump rescans immediately.
            last_rescan: std::time::Instant::now() - RESCAN_INTERVAL,
        }
    }

    #[must_use]
    pub fn project_map(&self) -> &ControlMap {
        &self.mapper.project
    }

    /// Poll gamepads, rescan MIDI on a timer, and resolve+fire any pending
    /// events onto `cmd_tx`. Call once per engine tick, before draining
    /// `cmd_rx` (so anything it sends lands in the same tick).
    pub fn pump(&mut self, cmd_tx: &Sender<Command>) {
        self.pads.poll(&self.tx);
        if self.last_rescan.elapsed() >= RESCAN_INTERVAL {
            self.hub.rescan();
            self.last_rescan = std::time::Instant::now();
        }
        while let Ok(ev) = self.rx.try_recv() {
            if let Some((action, value)) = self.mapper.resolve(&ev) {
                if let Some(cmd) = to_command(&action, value) {
                    let _ = cmd_tx.send(cmd);
                }
            }
        }
    }

    /// Offer a key event to the mapping layer. Returns `true` if this exact
    /// key+modifiers combination has *any* binding — including a masking
    /// `Action::Nothing` — meaning the caller's built-in default for this
    /// key must be suppressed. `repeat` gates only whether a fresh press is
    /// resolved (a trigger should fire once, not once per repeat tick); the
    /// consumed signal itself ignores `repeat` so a held mapped key doesn't
    /// leak through to the built-in on its repeat events.
    #[allow(clippy::too_many_arguments)]
    pub fn offer_key(
        &mut self,
        key: &str,
        ctrl: bool,
        alt: bool,
        shift: bool,
        cmd: bool,
        repeat: bool,
        cmd_tx: &Sender<Command>,
    ) -> bool {
        let source = ControlSource::Key { key: key.to_string(), ctrl, alt, shift, cmd };
        if !self.mapper.has_binding(&source) {
            return false;
        }
        if !repeat {
            let ev = ControlEvent { source, value: EventValue::Pressed };
            if let Some((action, value)) = self.mapper.resolve(&ev) {
                if let Some(c) = to_command(&action, value) {
                    let _ = cmd_tx.send(c);
                }
            }
        }
        true
    }
}

/// `Action -> Command`. `value` (normalized `0..=1`) only matters for
/// `SetBpm`; every other variant carries its own params.
fn to_command(action: &Action, value: f32) -> Option<Command> {
    match action {
        Action::Nothing => None,
        Action::TapDownbeat => Some(Command::TapDownbeat),
        Action::TapTempo => Some(Command::TapTempo),
        Action::SoftReset => Some(Command::SoftReset),
        Action::HardReset => Some(Command::HardReset),
        Action::CaptureShader => Some(Command::CaptureShader),
        Action::ToggleFullscreen => Some(Command::ToggleFullscreen),
        Action::SaveProject => Some(Command::SaveProject),
        Action::BpmDelta { amount } => Some(Command::BpmDelta(*amount)),
        Action::NudgeBpm { ratio } => Some(Command::NudgeBpm(*ratio)),
        Action::CycleLiveBank { delta } => Some(Command::CycleLiveBank(*delta)),
        Action::SetLiveBank { index } => Some(Command::SetLiveBank(*index as usize)),
        Action::SetEditBank { index } => Some(Command::SetEditBank(*index as usize)),
        Action::SetBpm { min, max } => {
            let bpm = min + (max - min) * f64::from(value);
            Some(Command::SetBpm(bpm.clamp(20.0, 1000.0)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_yields_no_command() {
        assert!(to_command(&Action::Nothing, 0.0).is_none());
    }

    #[test]
    fn trigger_actions_map_to_same_name_commands() {
        assert!(matches!(to_command(&Action::TapDownbeat, 1.0), Some(Command::TapDownbeat)));
        assert!(matches!(to_command(&Action::TapTempo, 1.0), Some(Command::TapTempo)));
        assert!(matches!(to_command(&Action::SoftReset, 1.0), Some(Command::SoftReset)));
        assert!(matches!(to_command(&Action::HardReset, 1.0), Some(Command::HardReset)));
        assert!(matches!(to_command(&Action::CaptureShader, 1.0), Some(Command::CaptureShader)));
        assert!(matches!(
            to_command(&Action::ToggleFullscreen, 1.0),
            Some(Command::ToggleFullscreen)
        ));
        assert!(matches!(to_command(&Action::SaveProject, 1.0), Some(Command::SaveProject)));
    }

    #[test]
    fn parameterized_triggers_carry_their_params() {
        assert!(matches!(
            to_command(&Action::BpmDelta { amount: 2.0 }, 1.0),
            Some(Command::BpmDelta(a)) if a == 2.0
        ));
        assert!(matches!(
            to_command(&Action::NudgeBpm { ratio: 0.01 }, 1.0),
            Some(Command::NudgeBpm(r)) if r == 0.01
        ));
        assert!(matches!(
            to_command(&Action::CycleLiveBank { delta: -1 }, 1.0),
            Some(Command::CycleLiveBank(d)) if d == -1
        ));
        assert!(matches!(
            to_command(&Action::SetLiveBank { index: 3 }, 1.0),
            Some(Command::SetLiveBank(i)) if i == 3
        ));
        assert!(matches!(
            to_command(&Action::SetEditBank { index: 2 }, 1.0),
            Some(Command::SetEditBank(i)) if i == 2
        ));
    }

    #[test]
    fn set_bpm_lerps_value_between_min_and_max() {
        let cmd = to_command(&Action::SetBpm { min: 60.0, max: 180.0 }, 0.5);
        assert!(matches!(cmd, Some(Command::SetBpm(b)) if (b - 120.0).abs() < 1e-9));
    }

    #[test]
    fn set_bpm_clamps_out_of_range_lerp() {
        let cmd = to_command(&Action::SetBpm { min: 60.0, max: 180.0 }, -10.0);
        assert!(matches!(cmd, Some(Command::SetBpm(b)) if (b - 20.0).abs() < 1e-9));
    }
}
