//! Save chat with file: the desktop app's assistant conversation about a document, written into
//! its native file (`.vectorcraft`, `.vctemplate`) when the person turns it on.
//!
//! The conversation is a top-level document key, [`KEY`], which loads back into
//! `Document::extra` like any key this version doesn't know. The desktop app sets a [`ChatHook`];
//! a save puts the conversation it gives into the copy of the document it writes (or takes the key
//! out when there is none), never into the open document.

use std::sync::{Arc, OnceLock};

use serde_json::Value;
use vectorcraft_doc::Document;

/// The document key that holds the conversation.
pub const KEY: &str = "artcraftAssistant";

/// The conversation to save in the native file at a path (None: no conversation, or the option off).
pub type ChatHook = Box<dyn Fn(&str) -> Option<Value> + Send + Sync>;

static HOOK: OnceLock<ChatHook> = OnceLock::new();

/// Sets the [`ChatHook`] (once per run).
pub fn set_hook(hook: ChatHook) {
    let _ = HOOK.set(hook);
}

/// The conversation saved in `doc`, if any.
pub fn chat_in(doc: &Document) -> Option<&Value> {
    doc.extra.get(KEY)
}

/// `doc`, about to be written to `path`, with the conversation the hook gives for it. Without a
/// hook (automation, the web app) the document is written as it is.
pub(crate) fn apply(path: &str, doc: &mut Arc<Document>) {
    let Some(hook) = HOOK.get() else { return };
    let want = hook(path);
    if chat_in(doc) != want.as_ref() {
        let d = Arc::make_mut(doc);
        match want {
            Some(chat) => {
                d.extra.insert(KEY.into(), chat);
            }
            None => {
                d.extra.remove(KEY);
            }
        }
    }
}
