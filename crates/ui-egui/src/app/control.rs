use craft::automation::ControlChannel;
use craft::automation::screenshot::{DocumentId, Finished, settle_ms};
use craft::ui::runtime::motion;
use craft::ui::runtime::windows::WindowHost;
use serde_json::json;
use vectorcraft_engine::file_access::AutomationRoots;

use super::{VectorcraftApp, file_access};
use crate::control;
use crate::services::WriteFn;

/// Writes a capture's PNG where the app's file authority allows: through the host's writer, inside
/// the automation roots. An error shows the `path` sent as its file name only when it lies outside
/// the read root, as every reply does (`hiding`).
fn write_automation_file(
    roots: Option<&std::sync::Arc<AutomationRoots>>,
    write: &mut Option<WriteFn>,
    hiding: &control::Hiding,
    path: &str,
    png: &[u8],
) -> Result<(), String> {
    file_access::confine(roots, || {
        let written = match write {
            Some(write) => write(path, png),
            None => Err("no writer configured".to_owned()),
        };
        written.map_err(|error| hiding.hide_error(&error, &json!({"path": path})))
    })
}

impl VectorcraftApp {
    /// Once per host iteration: opens the Claude pane's channel (from the assistant it started),
    /// then, on the control server's channel and the pane's in turn, answers the finished captures
    /// and runs the requests the transport queued, each inside the automation roots. The requests
    /// that wait for injected input are answered by [`Self::poll_input`].
    pub(crate) fn run_control(&mut self, ctx: &egui::Context) {
        // Open from the start and listed for the other Craft apps (once).
        self.assistant.share(ctx);
        // The assistant pane's own channel: requests from the assistant it started.
        if let Some(rx) = self.assistant.control_rx.take() {
            self.assistant_channel = Some(ControlChannel::new(rx));
        }
        self.each_channel(|app, pane, channel| app.run_channel(ctx, pane, channel));
        self.answer_pointer_waits(ctx);
    }

    /// The control server's channel (`pane` false) or the Claude pane's (`pane` true).
    fn channel_slot(&mut self, pane: bool) -> &mut Option<ControlChannel> {
        if pane { &mut self.assistant_channel } else { &mut self.channel }
    }

    /// The request of that channel held back until the synthetic input before it is applied.
    fn deferred_slot(&mut self, pane: bool) -> &mut Option<craft::automation::ControlRequest> {
        if pane { &mut self.assistant_deferred } else { &mut self.deferred_control }
    }

    /// Runs `f` on each open channel (the control server's, then the Claude pane's), with the
    /// channel taken out of the app while `f` runs; `f` is told which one it has.
    fn each_channel(&mut self, mut f: impl FnMut(&mut Self, bool, &mut ControlChannel)) {
        for pane in [false, true] {
            let Some(mut channel) = self.channel_slot(pane).take() else { continue };
            f(self, pane, &mut channel);
            *self.channel_slot(pane) = Some(channel);
        }
    }

    /// [`Self::run_control`] for one channel.
    fn run_channel(&mut self, ctx: &egui::Context, pane: bool, channel: &mut ControlChannel) {
        let host = WindowHost::of(ctx);
        let document = self.document_identity();
        channel.captures.note_windows(&host, document);
        let finished = channel.captures.drain(motion::now(), document);
        self.finish_captures(channel, finished);
        let settle = settle_ms(ctx.global_style().animation_time);
        while let Some(req) = self.deferred_slot(pane).take().or_else(|| channel.next_request()) {
            // A request held past its deadline is answered as one still queued then.
            if req.deadline.is_some_and(|deadline| motion::now() >= deadline) {
                let _ = req.reply.send(json!({"ok": false, "error": craft::automation::control::expired_error()}));
                continue;
            }
            // `ui.pointer` and `ui.tool.select` act at once, while `ui.key` and other synthetic input reach the tool
            // in a later pass: they wait for that earlier input, so a key pressed during a drag reaches its tool first.
            if matches!(req.method.as_str(), "ui.pointer" | "ui.tool.select") && !host.injected_idle() {
                *self.deferred_slot(pane) = Some(req);
                ctx.request_repaint();
                break;
            }
            // An automation request sees every plug-in the launch loads: what it reads or writes
            // (rendered artwork, file formats, effect lists) would otherwise depend on timing. It
            // waits in the queue while the frames install them, so the window keeps drawing; one
            // held past its deadline meanwhile is answered above as expired. The frame's poll of
            // the loads asks for the next frame.
            if self.session.plugins_loading() {
                self.deferred_control = Some(req);
                break;
            }
            let roots = self.automation_roots.clone();
            self.ui.answering_control = true;
            // Guarded per request: a panic must not stop the requests behind it.
            let answer = file_access::confine(roots.as_ref(), || vectorcraft_engine::guard::catch_panic(|| control::handle(self, ctx, &req)));
            self.ui.answering_control = false;
            let reply = match answer {
                Ok(reply) => reply,
                Err(msg) => {
                    self.session.set_replaying(false);
                    control::err(format!("internal error: {msg} (please report this bug)"))
                }
            };
            let document = self.document_identity();
            channel.settle(req, reply, document, settle);
        }
        // A capture is issued only after passes settle it, and an idle window runs none; a hidden
        // one runs only this logic unless asked to paint. Its readback finishes only in a later frame,
        // so frames keep coming until it is answered, as production's did.
        if channel.captures.any_pending() {
            if self.viewport_hidden(ctx) {
                host.paint_while_hidden();
            }
            ctx.request_repaint();
        }
    }

    /// The UI phase of a pass egui keeps: counts it for settling and issues the captures that are
    /// due, answering those that expired or lost their window meanwhile.
    pub(crate) fn issue_captures(&mut self, ctx: &egui::Context) {
        let document = self.document_identity();
        self.each_channel(|app, _, channel| {
            channel.captures.note_pass();
            let finished = channel.captures.issue(&WindowHost::of(ctx), motion::now(), document);
            app.finish_captures(channel, finished);
        });
    }

    /// Answers the requests that waited for injected input once the window host has run it, or
    /// fails them when it withdrew the input. Called at the end of a pass, after its UI, or between
    /// passes: called earlier in a pass, it would count the step the pass carries as run, and the
    /// rest of the pass would no longer read as injected (`WindowHost::injected_pass`).
    pub(crate) fn poll_input(&mut self, ctx: &egui::Context) {
        self.each_channel(|_, _, channel| channel.poll_input(&WindowHost::of(ctx)));
    }

    /// Answers the `ui.pointer` replies held on the tool's edit ([`control::PointerWait`]). The canvas
    /// checks on the tool each pass it is drawn; this checks too, for passes without it.
    fn answer_pointer_waits(&mut self, ctx: &egui::Context) {
        // Those whose events the canvas's check made go first, before this check makes the input
        // held after them.
        control::answer_pointer_waits(self);
        if self.pointer_waits.is_empty() {
            return;
        }
        if self.session.tool_editing() {
            let r = self.session.tool_poll(self.view_info(), false);
            crate::canvas::apply_requests(self, r);
        }
        control::answer_pointer_waits(self);
        if !self.pointer_waits.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    /// Replies to finished captures on `channel`; a capture with a path is written by [`write_automation_file`].
    fn finish_captures(&mut self, channel: &mut ControlChannel, finished: Vec<Finished>) {
        if finished.is_empty() {
            return;
        }
        let hiding = control::deferred_hiding(self);
        let roots = self.automation_roots.clone();
        let write = &mut self.services.write;
        channel.finish_captures(finished, &mut |path, png| write_automation_file(roots.as_ref(), write, &hiding, path, png));
    }

    /// The active document's identity: its process-unique uid and revision. A capture records it
    /// at admission and issuance and refuses to answer with a different document or undo state.
    pub(super) fn document_identity(&self) -> Option<DocumentId> {
        self.session.active().map(|document| DocumentId::new(document.uid, document.revision))
    }
}
