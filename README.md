# Panelink

Panelink is a planned Rust terminal multiplexer and persistent workspace runtime for Apple Silicon Macs. Run it inside an existing terminal application to manage OpenCode sessions and ordinary processes. Panelink's original application code is intended for the MIT license.

Current phase: research, product discovery, and a working specification. There is no application implementation yet. The interface is a terminal UI; the earlier desktop GUI hypothesis has been superseded. Some workflow and integration choices remain open.

- [Research and architecture recommendation](docs/PANELINK-RESEARCH.md)
- [Product questions and spec handoff plan](docs/PANELINK-DISCOVERY.md)
- [Working implementation specification](docs/SPEC.md)

Future implementers should read these documents before starting. Confirmed user requirements, engineering proposals, and unresolved decisions are distinguished. Dependencies require a concrete justification for saved effort and maintenance; there is no blanket approval to add libraries.
