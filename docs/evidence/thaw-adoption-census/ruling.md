# Thaw adoption ruling — issue 139

The ruling issue 140 reads. One row per raw HTML kind `web-ui/src` can author:
`replace` (140 swaps it for a Thaw primitive), `keep-native` (a native control
whose contract a Thaw primitive cannot carry), `no-equivalent` (semantics no
Thaw primitive emits, so the element stays).

Machine-readable regions: `thaw-census:kinds` and `thaw-census:worklist`.
Everything outside them is prose. `web-ui/scripts/thaw-adoption-census.mjs`
parses both; a row whose shape, class or attribute list is off is a parse error
(exit 2), not a silent skip.

The probe (`probe-table-progress.md`, HEAD `d58a94f`, bundle sha
`1332238b5efa`) measured `attr:` forwarding: `probe-table: landed`,
`probe-progress: landed`, `probe-progress-label: landed`. The six
probe-dependent rows below are the landed branch. Had the table probe dropped,
the table cluster would be `keep-native` (`stats-daily-table` and
`stats-shards-table` cannot be carried) and `thead`/`tbody`/`tr`/`td`
`no-equivalent`; had the progress label dropped, `progress` would be
`no-equivalent` (the accessible name would be lost).

<!-- thaw-census:kinds:begin -->
| kind | class | attrs-allowed | reason | basis |
| --- | --- | --- | --- | --- |
| ul | no-equivalent | - | list role; no Thaw list primitive; a div swap drops list semantics | READ web-ui/src ul sites |
| li | no-equivalent | - | list-item role; no Thaw list primitive | READ web-ui/src li sites |
| dl | no-equivalent | - | description-list semantics; no Thaw equivalent; authored dl sits inside Thaw Cards | READ screens.rs:213 |
| dt | no-equivalent | - | term semantics; no Thaw equivalent | READ core_screens.rs:2881 |
| dd | no-equivalent | - | description semantics; no Thaw equivalent | READ core_screens.rs:2882 |
| section | no-equivalent | - | region landmark with an accessible name; Card root is div role=group | READ card/mod.rs:17 |
| main | no-equivalent | - | main landmark no Thaw primitive emits | READ web-ui/src main sites |
| nav | no-equivalent | - | navigation landmark no Thaw primitive emits | READ web-ui/src nav sites |
| form | no-equivalent | - | form semantics no Thaw primitive emits | READ web-ui/src form sites |
| a | no-equivalent | - | Link root is a role=link anchor; the redundant role adds nothing | READ link/link.rs:44 |
| h1 | no-equivalent | - | heading semantics; Text adds classes and typography, not semantics | READ web-ui/src h1 sites |
| h2 | no-equivalent | - | heading semantics; Text adds classes and typography, not semantics | READ web-ui/src h2 sites |
| h3 | no-equivalent | - | heading semantics; Text adds classes and typography, not semantics | READ web-ui/src h3 sites |
| h4 | no-equivalent | - | heading semantics; Text adds classes and typography, not semantics | READ web-ui/src h4 sites |
| code | no-equivalent | - | inline code semantics; no Thaw equivalent | READ web-ui/src code sites |
| strong | no-equivalent | - | strong importance semantics; no Thaw equivalent | READ web-ui/src strong sites |
| p | no-equivalent | - | paragraph semantics; Body1 adds typography only | READ web-ui/src p sites |
| span | no-equivalent | - | same element; Body1 and Layout add classes, not semantics | READ web-ui/src span sites |
| div | no-equivalent | - | same element; Layout adds layout classes, not semantics | READ web-ui/src div sites |
| th | no-equivalent | - | TableHeaderCell wraps children in a button role=presentation and has no scope prop | READ table/mod.rs:23 |
| label | keep-native | - | for= pairing with native controls; Label has no for prop and Field pins its own uuid for | READ core_screens.rs:899 |
| select | keep-native | - | three live testid contracts: agents-project-select (agents.rs:595), sessions-project-select (screens.rs:735), profile-identity-select (core_screens.rs:1690); Select roots in a span so data-testid lands on the wrapper | READ select/select.rs:63 |
| option | keep-native | - | options live inside the three native selects: agents-project-select (agents.rs:866), sessions-project-select (screens.rs:748), profile-identity-select (core_screens.rs:1696,1709); no Thaw option primitive | READ core_screens.rs:1696 |
| input | keep-native | - | skill-form-autoinvoke and skill-form-enabled are checkboxes each wrapped by its own label (implicit association) with a name; Checkbox roots in a span and has no name prop | READ skill_library.rs:699,706 |
| textarea | keep-native | - | import-input and skill-form-content; Textarea roots in a span and its attrs prop is commented out | READ textarea/mod.rs:47 |
| table | replace | class,data-testid | Table takes class only; attr:data-testid reaches the TABLE root (probe); 140 must keep test-wave6-cdp.cjs:183 passing | PROBE probe-table |
| thead | replace | class | TableHeader wrapper takes class only; part of the landed table cluster | PROBE probe-table |
| tbody | replace | class | TableBody wrapper takes class only; part of the landed table cluster | PROBE probe-table |
| tr | replace | class | TableRow wrapper takes class only; part of the landed table cluster | PROBE probe-table |
| td | replace | class | TableCell wrapper takes class only; part of the landed table cluster | PROBE probe-table |
| progress | replace | aria-label,class,value,max | ProgressBar roots in div role=progressbar; attr:aria-label forwards the name; value and max map to count as f64 and peak as f64 | PROBE probe-progress |
<!-- thaw-census:kinds:end -->

## Worklist for issue 140

Every `replace` occurrence, counted per file. 20 in one file. 140 shrinks these
to zero; it must also delete the `.stats-progress` rule (`public/styles.css:1174`)
and its doc comment (`:1056`) in the same change, else `check-class-coverage`
reports `dead>0`.

<!-- thaw-census:worklist:begin -->
| file | kind | count |
| --- | --- | --- |
| core_screens.rs | table | 2 |
| core_screens.rs | thead | 2 |
| core_screens.rs | tbody | 2 |
| core_screens.rs | tr | 4 |
| core_screens.rs | td | 9 |
| core_screens.rs | progress | 1 |
<!-- thaw-census:worklist:end -->

## Notes for 140

- `progress` edge: `peak` (`daily_peak`, `core_screens.rs:1969`) can be 0.
  Thaw's width is `value/max*100`, which is NaN at `max = 0`; native
  `<progress max=0>` is indeterminate. 140 must handle `peak = 0` explicitly.
- `table`: the acceptance check is
  `web-ui/scripts/test-wave6-cdp.cjs:183` (`[data-testid="stats-daily-table"]
  tbody tr`), which must keep passing.
- A `replace` row whose occurrence carries an attribute outside `attrs-allowed`
  fails the census (exit 1). Add the attribute to the row and to its
  `attrs-allowed` cell in the same edit, or leave the element native.
