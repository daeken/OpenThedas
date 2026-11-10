//! What the interface's messages do to the simulation: the player's input through the game's
//! own screens (picking a conversation reply, accepting a job, travelling on the world map).
//! Movies only send messages; the simulation acts on them here, as input, the way it acts on
//! a click in the world.

use std::sync::{Arc, Mutex};

use thedas_sim::Sim;

use crate::protocol::{GameMessage, MessageHandler};

/// conversation.gfx's reply pick: the reply's index.
pub const SELECT_RESPONSE: &str = "Conversation.SelectResponse";

/// Acts on a message a movie sent: a conversation reply is picked (continuing past the last
/// line of one that ended closes it), anything else goes to the simulation's interface
/// handlers. Returns whether the simulation acted on it.
pub fn apply(sim: &mut Sim, name: &str, args: &[String]) -> bool {
    if name == SELECT_RESPONSE {
        if sim.dialog_state().is_some_and(|d| d.ended) {
            sim.end_conversation();
        } else {
            let n = args
                .first()
                .and_then(|a| a.parse::<usize>().ok())
                .unwrap_or(0);
            sim.choose_reply(n);
        }
        sim.tick(0);
        return true;
    }
    if sim.gui_message(name, args) {
        sim.tick(0);
        return true;
    }
    false
}

/// Acts on the messages a tick returned that nothing handled yet; returns the ones the
/// simulation did not take (name and arguments).
pub fn apply_all(sim: &mut Sim, messages: &[GameMessage]) -> Vec<(String, Vec<String>)> {
    let mut ignored = Vec::new();
    for m in messages {
        if let GameMessage::Send(name, args) = m {
            if !apply(sim, name, args) {
                ignored.push((name.clone(), args.clone()));
            }
        }
    }
    ignored
}

/// A bridge handler acting on `sim` as a movie sends a message, before the movie goes on (a
/// movie reads values right after some messages, as chanters.gfx does after accepting a
/// job). When the simulation is busy elsewhere the message waits for [`apply_all`].
pub fn handler(sim: Arc<Mutex<Sim>>) -> MessageHandler {
    Arc::new(move |name: &str, args: &[String]| {
        let mut sim = sim.try_lock().ok()?;
        // Replies are picked after the tick, as the MCP does: the movie does not read the
        // next line back in the same call.
        if name == SELECT_RESPONSE || !sim.gui_message(name, args) {
            return None;
        }
        sim.tick(0);
        Some(crate::binding::sim_values(&sim).into_iter().collect())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sim() -> Sim {
        Sim::new(
            Arc::new(thedas_vfs::Vfs::new()),
            Arc::new(thedas_script::EngineTable::default()),
            1,
        )
    }

    #[test]
    fn unknown_messages_are_passed_back() {
        let mut s = sim();
        let left = apply_all(
            &mut s,
            &[
                GameMessage::Send("Nobody.Listens".into(), vec!["1".into()]),
                GameMessage::Handled("Already.Done".into(), vec![]),
            ],
        );
        assert_eq!(left, vec![("Nobody.Listens".to_string(), vec!["1".into()])]);
    }

    #[test]
    fn the_handler_leaves_replies_for_after_the_tick() {
        let h = handler(Arc::new(Mutex::new(sim())));
        assert!(h(SELECT_RESPONSE, &["0".into()]).is_none());
    }
}
