# Disqro engineering

- Use American English in first-party code, interface copy, documentation, and diagnostics.
- Keep one current data model, persistence contract, and implementation per concern. Remove superseded paths, adapters, aliases, historical workspace readers, migrations, and feature flags when their replacements land. Update their callers and tests together.
- Preserve stable logical recording identities, independent browser state, and explicit ownership of asynchronous work. Share interface primitives across surfaces with the same interaction.
- Preserve Favorites, ratings, playlists, listening history, registered collections, and workspace settings. Create disposable profiles directly with the current schema. Refuse incompatible library databases without altering them. Handle actual personal data through an isolated, one-time conversion or export and import with the owner's explicit approval.
- Qualify Windows changes with isolated profiles, silent audio, and the private native desktop tools. Keep the Waveform Native dependency and Windows presentation patch pinned together.

## Source and interface contracts

- Use snake_case Rust module directories, filenames, functions, bindings, and fields. Use UpperCamelCase for types and variants, SCREAMING_SNAKE_CASE for constants, and kebab-case Cargo package names. Python tools follow snake_case. Keep historical references to external source and serialized identifiers exact.
- Follow the [GPUI Kit design guide](https://gpui-kit.com/versions/v0.10.0/docs/design-guides/) and [coding guide](https://gpui-kit.com/versions/v0.10.0/docs/coding-guides/). Check signatures and event routing against the locked crate source. Prefer Kit controls and the shared Materials palette, size tokens and disclosures. Keep physical-pixel geometry at native rendering boundaries.
- Give stateful controls stable domain and instance identities. Check name, role, selected, expanded and disabled states, keyboard focus and activation. Exercise nested controls with Enter and Space while global shortcuts are bound.
- Keep recurring subscriptions with the entity that needs them. Use weak entity handles in asynchronous UI callbacks. Bound work and payload residency, cancel superseded requests and check the active generation before publishing. Give every cached result an invalidation and release path.
- Audio rendering must avoid blocking, file I/O, heap allocation and unbounded traversal. Decode and prepare evidence on bounded workers. Keep transport position and waveform presentation on the shared clock.
- Review the final diff and commit coherent changes separately. Build production and qualification executables separately. Keep private engine access controlled and report hosted CI failures. Merge and release require explicit authorization.

## Development workflow

Choose a mission type and a focused outcome. For interface work, reference the [design specification](docs/design-foundations.md). The contracts above apply throughout.

| Mission | Evidence for completion |
| --- | --- |
| Development | Implement the requested user-visible change, run the smallest relevant regression checks and exercise the changed interaction in the native app. Use an incremental build. |
| Engineering review | Inspect the diff from its actual base. Reconcile saved results with the revision, build inputs and workload. Probe suspected regressions and qualify corrections where they can fail. |
| Integration qualification | At an integration or release milestone, run the locked suite, formatting, warnings-denied Clippy across all targets and features, source naming, American English checks, a production build and relevant private native journeys. Record revision, measured DPI, workload, memory, image comparisons and gaps. |

Use one agent by default. Expand validation when the blast radius or a failure warrants it. Preserve failed runs and repeat their affected checks. Reuse applicable evidence and warm builds. Repeat a benchmark batch or release build when it answers an outstanding question. [Windows development builds](docs/development-builds.md)

For work spanning sessions, keep one short handoff with the branch, revision, objective, decisions, completed work, evidence, open failures, deferred items and exact next action. Update it before an expensive run and when a meaningful result changes the next action. Stop at the mission’s acceptance criteria. [Current design review handoff](docs/design-review-handoff.md)

## GitHub spending and merge checks

- Keep GitHub spending at zero. Use only free capacity. Do not purchase subscriptions, additional Actions minutes, or other paid resources.
- Hosted CI supplements local qualification when free capacity is available. Check the result of each job before integration. An actual compilation, Clippy, test, or qualification failure blocks the merge.
- If GitHub blocks a job before execution because of billing, record the run link and the billing diagnostic in the PR. Run the equivalent local checks on the exact reviewed revision and record its commit, tree, commands, and results. Revalidate affected checks if the revision changes.
- Report hosted and local results separately. Passing local evidence on the reviewed revision satisfies a billing-blocked check. This standing exception needs no additional billing approval or job rerun. Preserve independent review, merge authorization, and unrelated branch protections.
- Keep Waveform Native private and preserve the existing read-only dependency authentication and trusted integration branch restrictions.
