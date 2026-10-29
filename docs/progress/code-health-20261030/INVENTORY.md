# Combined source inventory and design review

Compare the selected integration base a1134ec with packet05 source2b72e81. Counts below are whole files, including comments and fixture adapters. They do not treat the first cfg(test) attribute as a production cutoff.

UI state.rs7593→4040 lines. Private viewport701, workspace1512, copy639, interaction1437 and render571 own their transitions/presentation. Their combined8900 lines exceed the original monolith; explicit context/feedback interfaces have a cost. The benefit is that history scheduling, metadata identity, copy and confirmation changes each have a defined owner and immutable read boundary. State scenarios remain in state_tests.rs; no duplicate replacement suite was added.

Server multi_session.rs9934→3878 lines. Extracted tests.rs3184, split_tests.rs1757 and runtime_helper_tests.rs1099 are test-only modules, preserving test names and assertions. Existing async_split/close modules stay in place. The authority core remains together. Compositor compose.rs4418 lines is unchanged; shared chrome geometry and its substantial embedded tests were already coherent.

Parent reviewed the combined boundaries and each packet's evidence: deleting viewport/workspace/copy/interaction would spread its actual policy back into callers; deleting render would reunite frame assembly with business coordination. These are concrete owners/assembly boundaries, not helpers taking whole mutable App. Cross-domain admission, pending operations, route freeze, output acceptance and pointer extent coordination remain centralized. Root event routing and frame assembly are still long methods, and CLI/runtime authority refactors remain deferred.

Changed production paths are host dispatch and private UI ownership/rendering; manifests, lockfile, protocol/JSON/storage types and network adapters are unchanged. Test synchronization fixes are separately documented in baseline/02/06 evidence, with original failures retained. Runtime usage guidance remains unchanged; the repository skill adds only the navigation link. Skill frontmatter/navigation and repository Markdown links validate. No installed artifact or live process was operated on.

Focused checks pass as recorded in packets01–07. The fresh [combined canonical gate](INTEGRATION.md) passes8/8 and parent checked its results. Independent viewport/workspace reviews completed; interaction review was completed by the parent after Luna stopped incomplete. No independent interaction approval or native/provider/performance acceptance is claimed.

State behavioral tests6080→6117 lines reflect import/owner-path changes and formatting, preserving scenarios/assertions; no mirror suite was added. Host1557→1556 lines centralizes duplicate dispatch. Counts include test-only adapters; the separated server children are wholly test modules, while UI state children contain production behavior.
