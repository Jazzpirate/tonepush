//! Moving a preset to another place in the list, the presets between
//! closing up behind it, as dropping one between two others does in HX
//! Edit.
//!
//! No command for it is known, so the move writes every slot from where the
//! preset was to where it goes, each with what it holds afterwards. The
//! documents come from the automatic backup, which holds every slot that is
//! not empty, and all of them are read before anything is written. An empty
//! slot moves like any other: the slot it lands in is emptied. The backup TonePush refreshes
//! afterwards sets the one from before aside, so a run of writes that stops
//! partway loses nothing.

use crate::dnd::Outcome;
use crate::put::{Asking, Write};
use crate::session::Cmd;
use crate::theme::Icon;
use crate::{library, App};

/// Where a preset dropped in the gap before `before` ends up.
pub(crate) fn destination(from: i64, before: i64) -> i64 {
    if before > from {
        before - 1
    } else {
        before
    }
}

/// Where the preset in `slot` is once the one in `from` has moved to `to`.
pub(crate) fn moved(slot: i64, from: i64, to: i64) -> i64 {
    if slot == from {
        to
    } else if from < to && (from + 1..=to).contains(&slot) {
        slot - 1
    } else if to < from && (to..from).contains(&slot) {
        slot + 1
    } else {
        slot
    }
}

/// What a slot holds, for a move: its name and document, or nothing.
pub(crate) type Held = Option<(String, Vec<u8>)>;

/// Every slot a move writes, with what it holds after, in slot order. Fails
/// with the first slot whose contents are not known.
pub(crate) fn plan(
    from: i64,
    to: i64,
    mut held_in: impl FnMut(i64) -> Option<Held>,
) -> Result<Vec<(i64, Held)>, i64> {
    let (low, high) = (from.min(to), from.max(to));
    let mut held = Vec::new();
    for slot in low..=high {
        held.push(held_in(slot).ok_or(slot)?);
    }
    if from < to {
        held.rotate_left(1);
    } else {
        held.rotate_right(1);
    }
    Ok((low..=high).zip(held).collect())
}

impl App {
    /// What dropping the preset in `from` in the gap before `before` does.
    pub(crate) fn reorder_outcome(&self, from: i64, before: i64) -> Option<Outcome> {
        let to = destination(from, before);
        if to == from || before < 0 || before > self.slot_count() {
            return None;
        }
        if self.pro_active() {
            return Outcome::refuses("Presets move this way on HX pedals only");
        }
        if let Some(why) = self.put_refusal() {
            return Outcome::refuses(why);
        }
        if let Err(slot) = plan(from, to, |slot| self.held_for_move(slot)) {
            return Outcome::refuses(self.not_to_hand(slot));
        }
        let between = (to - from).abs();
        let shift = if from < to { "up" } else { "down" };
        Outcome::does(
            Icon::ArrowDownToLine,
            format!(
                "Move to {}: {between} {} shift {shift}",
                self.active_slot_label(to),
                if between == 1 { "preset" } else { "presets" },
            ),
        )
    }

    /// What a slot holds, out of the automatic backup: a slot it has no file
    /// for is empty when the backup lists it without a name. `None` when that
    /// is not known, or when the slot is loaded with changes not saved.
    fn held_for_move(&self, slot: i64) -> Option<Held> {
        if let Some(document) = self.slot_document(slot) {
            return Some(Some(document));
        }
        if self.loaded_slot_index() == Some(slot) && self.dirty {
            return None;
        }
        let names = &self.automatic_backup.as_ref()?.presets;
        names
            .get(usize::try_from(slot).ok()?)
            .is_some_and(|name| name.is_empty())
            .then_some(None)
    }

    /// Why a slot's document cannot be read for a move.
    fn not_to_hand(&self, slot: i64) -> String {
        let label = self.active_slot_label(slot);
        if self.loaded_slot_index() == Some(slot) && self.dirty {
            format!("{label} has changes not saved: save it first")
        } else {
            format!("{label} is not in the backup yet: wait for it")
        }
    }

    /// Ask before moving the preset in `from` to the gap before `before`.
    pub(crate) fn ask_reorder(&mut self, from: i64, before: i64) {
        let to = destination(from, before);
        if to == from {
            return;
        }
        let writes = match plan(from, to, |slot| self.held_for_move(slot)) {
            Ok(writes) => writes,
            Err(slot) => return self.problem(self.not_to_hand(slot)),
        };
        self.put_question = Some(Asking {
            writes: writes
                .into_iter()
                .map(|(slot, held)| {
                    let (name, bytes) = held.unzip();
                    Write {
                        slot,
                        label: self.active_slot_label(slot),
                        hash: bytes.as_deref().map(library::hash_of).unwrap_or_default(),
                        name: name.unwrap_or_default(),
                        replaces: None,
                        bytes,
                    }
                })
                .collect(),
            beside: None,
            back_up_first: false,
            clears: None,
            reorder: Some((from, to)),
        });
    }

    /// Write a move that was agreed, and keep playing what was playing,
    /// where it went.
    pub(crate) fn write_reorder(&mut self, writes: Vec<Write>, from: i64, to: i64) {
        self.put_back();
        let name = writes
            .iter()
            .find(|write| write.slot == to)
            .map(|write| write.name.clone())
            .unwrap_or_default();
        // A write with no document empties its slot: an empty one moved.
        let slots = writes
            .into_iter()
            .map(|write| (write.slot, write.bytes.map(|bytes| (write.name, bytes))))
            .collect();
        let words = format!("Moved {name} to {}", self.active_slot_label(to));
        self.note(words.to_lowercase());
        self.send(Cmd::PushSetlist(slots));
        self.wrote = Some((words, std::time::Instant::now()));
        if let Some(loaded) = self.loaded_slot_index() {
            let now = moved(loaded, from, to);
            if now != loaded {
                self.load_preset(now);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(slot: i64) -> Option<Held> {
        Some(Some((format!("P{slot}"), vec![slot as u8])))
    }

    #[test]
    fn a_preset_lands_in_the_gap_it_was_dropped_in() {
        // Down the list: the gap before 10 is slot 9 once 3 has left.
        assert_eq!(destination(3, 10), 9);
        // Up the list: the gap before 1 is slot 1.
        assert_eq!(destination(10, 1), 1);
        // Either gap beside it leaves it where it is.
        assert_eq!(destination(5, 5), 5);
        assert_eq!(destination(5, 6), 5);
    }

    #[test]
    fn the_presets_between_close_up_behind_it() {
        let names = |from: i64, to: i64| -> Vec<(i64, String)> {
            plan(from, to, named)
                .unwrap()
                .into_iter()
                .map(|(slot, held)| (slot, held.unwrap().0))
                .collect()
        };
        let expect = |pairs: [(i64, &str); 4]| pairs.map(|(s, n)| (s, n.to_owned())).to_vec();
        assert_eq!(
            names(1, 4),
            expect([(1, "P2"), (2, "P3"), (3, "P4"), (4, "P1")])
        );
        assert_eq!(
            names(4, 1),
            expect([(1, "P4"), (2, "P1"), (3, "P2"), (4, "P3")])
        );
        // Every preset is still somewhere, and each where `moved` says.
        for (from, to) in [(1, 4), (4, 1), (0, 127), (127, 0)] {
            for (slot, held) in plan(from, to, named).unwrap() {
                let was: i64 = held.unwrap().0[1..].parse().unwrap();
                assert_eq!(moved(was, from, to), slot);
            }
        }
    }

    #[test]
    fn nothing_is_planned_without_knowing_every_slot() {
        // An empty slot is known; one the backup cannot speak for is not.
        assert_eq!(plan(2, 6, |slot| (slot != 4).then_some(None)), Err(4));
        assert!(plan(2, 6, |_| Some(None)).is_ok());
    }

    #[test]
    fn the_presets_outside_the_move_stay_put() {
        assert_eq!(moved(0, 3, 9), 0);
        assert_eq!(moved(10, 3, 9), 10);
        assert_eq!(moved(9, 3, 9), 8);
        assert_eq!(moved(3, 9, 3), 4);
    }

    #[test]
    fn a_move_writes_its_slots_and_what_plays_follows_it() {
        let (to_device, cmds) = std::sync::mpsc::channel();
        let (_events, from_device) = std::sync::mpsc::channel();
        let mut app = App::new(&egui::Context::default(), to_device, from_device);
        let _ = cmds.try_iter().count();
        app.device = "HX Stomp".into();
        app.connection = crate::Connection::Online;
        app.presets = (0..8).map(|slot| format!("P{slot}")).collect();
        // P4 plays; P1 moves to before slot 6, so P4 slides up to 3. Slot 3
        // is empty, and stays empty as it moves up to 2.
        app.preset_index = 4;
        let writes = plan(
            1,
            5,
            |slot| if slot == 3 { Some(None) } else { named(slot) },
        )
        .unwrap()
        .into_iter()
        .map(|(slot, held)| {
            let (name, bytes) = held.unzip();
            Write {
                slot,
                label: String::new(),
                hash: String::new(),
                name: name.unwrap_or_default(),
                replaces: None,
                bytes,
            }
        })
        .collect();
        app.write_reorder(writes, 1, 5);
        let sent: Vec<Cmd> = cmds.try_iter().collect();
        let written: Vec<(i64, Option<String>)> = sent
            .iter()
            .find_map(|cmd| match cmd {
                Cmd::PushSetlist(slots) => Some(
                    slots
                        .iter()
                        .map(|(slot, held)| (*slot, held.as_ref().map(|(name, _)| name.clone())))
                        .collect(),
                ),
                _ => None,
            })
            .expect("the slots are written");
        let p = |name: &str| Some(name.to_owned());
        assert_eq!(
            written,
            [
                (1, p("P2")),
                (2, None),
                (3, p("P4")),
                (4, p("P5")),
                (5, p("P1"))
            ]
        );
        assert!(
            matches!(sent.last(), Some(Cmd::SelectPreset(3))),
            "what played is loaded where it went"
        );
    }
}
