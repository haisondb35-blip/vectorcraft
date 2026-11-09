//! Save chat with file: the assistant's conversation about a document, written into its native
//! file (`.vectorcraft`, `.vctemplate`) when the person turns it on (off by default), so it travels
//! with the file and comes back when the file is opened on another computer.
//!
//! The conversation (portable form, without this computer's conversation id, see
//! `Memory::to_json`) is a top-level document key that other versions keep as a key they don't
//! know (`vectorcraft_engine::assistant_chat`). Saving with the option off takes it out again.
//! Other formats (SVG, PDF, AI…) are saved as they are.

use serde_json::Value;

/// The largest file read whole to find its conversation (opening a file).
const MAX_FILE: u64 = 256 << 20;

/// Files the conversation can be saved in.
pub fn can_embed(path: &str) -> bool {
    let ext = std::path::Path::new(path).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    matches!(ext.as_str(), "vectorcraft" | "vctemplate")
}

/// Has saves put the conversation `a` keeps for each file into it, and `a` read it back.
pub fn install(a: &mut super::Assistant) {
    let shelf = a.shelf.clone();
    vectorcraft_engine::assistant_chat::set_hook(Box::new(move |path| {
        if !can_embed(path) {
            return None;
        }
        shelf.lock().ok().and_then(|s| s.chat_for(path))
    }));
    a.embedded_reader = Some(read);
}

/// The conversation saved in the file at `path`, if it has one.
pub fn read(path: &str) -> Option<Value> {
    if !can_embed(path) || std::fs::metadata(path).ok()?.len() > MAX_FILE {
        return None;
    }
    let doc = vectorcraft_format::load(&std::fs::read(path).ok()?).ok()?;
    vectorcraft_engine::assistant_chat::chat_in(&doc).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_chat_in_the_document_survives_the_native_file() {
        let chat = json!({"version": 1, "messages": [{"role": "user", "text": "Vẽ logo tròn"}]});
        let mut doc = vectorcraft_engine::doc::Document::new(100.0, 50.0);
        doc.extra.insert(vectorcraft_engine::assistant_chat::KEY.into(), chat.clone());
        let back = vectorcraft_format::load(&vectorcraft_format::save_file(&doc)).unwrap();
        assert_eq!(vectorcraft_engine::assistant_chat::chat_in(&back), Some(&chat));
        assert!(can_embed("C:/Thiết kế/Logo.VectorCraft") && can_embed("a.vctemplate") && !can_embed("a.svg"));
    }
}
