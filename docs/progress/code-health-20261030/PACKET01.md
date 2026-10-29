# Packet01: unified host event dispatch

The private dispatcher now resolves recovery availability, handles one event and drains create/mutation/route/catalog intents once, in existing order on Continue. Quit and ViewRecovery return before ordinary-event draining; the delayed lone-Escape path keeps its documented cannot-open-viewer behavior. App interface, worker-result processing, deadlines, input budget, rendering and restoration are unchanged.

Parent reviewed the entire diff and integrated only host.rs. Worker verified host unit6/0, strict bin Clippy, attach-host6/0, default-open8/0/1ignored and fmt/diff checks. Its final report described workspace tests as stalled, but the raw task activity contains completed5/0 results for those commands. Parent independently verified integrated host unit6/0 and workspace target5/0 in36.32s. Logs: `/private/tmp/panelink-code-health-01-unit-parent.log`, `/private/tmp/panelink-code-health-01-parent.log`. There is no pending worker test process or observed failure to suppress.

No runtime install or live pane use. Full combined canonical acceptance remains packet08.
