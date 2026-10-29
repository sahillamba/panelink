# Packet07: embedded server test organization

Parent reviewed and integrated the Luna worktree at base ce51e02. The three inline test modules now live in test children with original names and explicit paths. Production prefix is byte-identical. Parent extracted each original body, dedented it and ran rustfmt; each result exactly equals its new file. No helper/body/import/assertion change is hidden by the relocation. Existing async_split and close_tests files are unchanged.

Worker before/after test lists match exactly:935tests/0benchmarks; scoped library tests85/0. Parent independently verified body/production equivalence and ran the relocated suite85/0, log `/private/tmp/panelink-code-health-07-parent.log`. The large owner file is reduced from9934 to3878 physical lines; no production authority state machine was split.

No behavior, ownership, wire, storage, dependency, ignored-test or product acceptance change. Full combined canonical gate remains packet08.
