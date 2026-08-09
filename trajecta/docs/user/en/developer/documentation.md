---
title: Documentation contributions
description: Source layout, bilingual editing, executable examples, generated references, writing style, SEO, and publication checks.
---

# Documentation contributions

The Trajecta manual is a MkDocs Material site written in Markdown. It serves
scientific users first, then operators and contributors. Commands and document
formats come from the product; the site does not maintain a second interface
description by hand.

English is the normative source. The Chinese manual covers the same pages and
information while using natural Chinese technical prose.

## Source layout

```text
docs/
├── user/
│   ├── en/
│   └── zh-CN/
├── site-overrides/
└── engineering/
```

The two language directories have identical relative file paths. `mkdocs.yml`
contains one navigation tree; the internationalization plugin applies translated
navigation labels and builds both sites.

`docs/engineering` contains plans, execution reports, and historical prompts.
It is outside `docs_dir`, so those records do not enter navigation, site search,
or the sitemap. Link public pages to maintained product references rather than
to an engineering report that a site reader cannot open.

The user tree follows a task-oriented structure:

| Section | Reader question |
| --- | --- |
| Getting Started | How do I install, configure, and obtain a first verified result? |
| Tutorials | How does a complete scientific workflow fit together? |
| How-to Guides | How do I complete one operational task? |
| Concepts | What do the product objects and scientific models mean? |
| Operations | How do I run and recover work on a real machine? |
| Validation | How were scientific comparability and performance measured? |
| Reference | What exact commands, fields, formats, states, and limits exist? |
| Developer Guide | How is the implementation built, tested, and released? |
| Releases | What changed in a named software version? |

Place a page according to the question it answers. A tutorial may link to a
concept and a reference table instead of repeating their full content.

## Page shape

Every Markdown page begins with front matter:

```yaml
---
title: Short page title
description: One sentence that identifies the page's subject and useful scope.
---
```

The page then has one level-one heading matching the title. Use level-two
headings for the main reading path and level-three headings for subdivisions.
Avoid skipping heading levels.

A practical page usually contains:

1. a short opening that says what the reader can do or understand;
2. prerequisites or scope when they affect the procedure;
3. the procedure, model, or table itself;
4. the expected result and the next relevant command;
5. links to deeper concepts or exact reference material.

Length follows the subject. A diagnostic index can be compact; a build or
recovery guide needs enough detail to carry a reader through a real session.

## Bilingual editing

For each English path, create or update the same path under `zh-CN`. Heading
levels must have the same sequence so links and navigation remain comparable.
Paragraph count and sentence order may differ when Chinese reads better with a
different grouping.

Translate meaning and product terminology consistently:

| Product term | Chinese treatment |
| --- | --- |
| Case, RunProfile, DatasetLock | Keep the type name; explain it in Chinese at first use |
| Project, profile, attempt, worker | Keep the product term where it matches CLI or disk fields |
| Forward/backward trajectory | Use 正向/反向轨迹, retaining the command value in code |
| Domain filling | Introduce as domain filling（区域填充）, then use the term that fits the page |
| Diagnostic code, schema, manifest, provenance | Keep exact machine tokens in code; explain their role in Chinese |

Do not translate command names, option names, JSON keys, schema IDs, file names,
or diagnostic codes. A Chinese sentence can describe their effect around the
literal token.

Release publication requires both languages to be complete. A short-lived dev
change may carry an explicit translation update in the same branch, but the
validator still requires both paths and heading shapes.

## Writing style

Write as a technical maintainer addressing a reader who has real work to do.
State definitions and conditions directly. Use the program's terms, show the
command, and describe the observable result.

### English

- Prefer concrete subjects: “The daemon writes the event” is clearer than “An
  event will be written.”
- Keep a paragraph around one idea.
- Use a table when several options have the same fields.
- Expand an uncommon abbreviation at first use.
- Reserve requirements language for an actual contract or safety condition.
- Avoid promotional adjectives and unmeasured performance claims.

### Chinese

- Use the tone of a scientific software manual, with ordinary sentence rhythm.
- Define the object before listing operations on it.
- Split long condition chains into a table or short list.
- Avoid formulaic contrast constructions and strings of enumeration marks.
- Keep code tokens in code formatting; surrounding prose should remain readable
  without mentally translating every English noun.
- Describe current behavior without defensive disclaimers or commentary about
  what a test can prove.

User pages explain how the software behaves. Acceptance history, internal
milestone ownership, and review rhetoric belong in engineering records.

## Commands and executable examples

Before documenting a command, run its `--help` from the current binary. Use the
option order accepted by that parser and show a path form that works on both
supported platforms where possible.

Command blocks intended for copying begin with the product executable:

```text
trajecta --project examples/domain-fill-cfsr project validate
```

Do not include a shell prompt marker in the same block. Readers can then copy the
line into PowerShell, Bash, or an automation file. Add a separate platform block
only when syntax genuinely differs, such as environment-variable assignment.

Complete tutorial projects live under `examples`. Their Case, Profile, and
project index are the configuration source of truth. Include them with the
snippets extension:

```text
--8<-- "examples/domain-fill-cfsr/cases/moisture.yaml"
```

The documentation validator allows user snippets only from `examples/`. Editing
the example therefore changes the tutorial input used by automated quickstarts
and prevents a stale hand-copied YAML block.

When a procedure creates output, name the exact file or machine field that the
reader should see. Avoid fixed run IDs, absolute home directories, and timestamps
that are generated at runtime.

## Generated reference pages

`tools/generate_m5_1_reference.py` generates six bilingual pages covering the
CLI tree, product schemas, and diagnostic index. It reads:

- `trajecta --help` from a real binary;
- `testdata/M5_CLI_CONTRACT.v1.json`;
- public JSON schemas and examples under `testdata`;
- diagnostic string literals from `trajecta-cli` and `trajecta-job` source.

Generated pages carry this marker:

```text
<!-- Generated by tools/generate_m5_1_reference.py; do not edit. -->
```

!!! tip "Edit the source, not the generated page"

    Change the command contract, schema, diagnostic source, or generator, then
    regenerate the page.

Run:

```text
python tools/generate_m5_1_reference.py --binary target/debug/trajecta-cli
```

Use `target/debug/trajecta-cli.exe` on Windows. CI runs check mode:

```text
python tools/generate_m5_1_reference.py \
  --binary target/debug/trajecta-cli --check
```

The generator's command-purpose table provides concise user-facing descriptions.
It should describe what a command does and the form of its output, without
exposing milestone labels or code ownership.

Hand-written reference introductions explain semantics, composition, and risk.
They do not duplicate every field already rendered from a schema.

## Links and sources

Use relative links for pages and assets inside the manual. The validator checks
that each target file exists. Link to a section when it helps the reader land on
the exact rule.

Scientific claims should link directly to an authoritative source such as a data
provider, peer-reviewed method, or official format specification. Keep the link
near the claim. A separate bibliography page is unnecessary for the current
manual.

External links run through a retrying CI checker. Choose stable provider or
project URLs and avoid session-specific download links.

The compared model's name is confined to Validation pages. Home, tutorial, and
ordinary reference pages describe Trajecta on its own terms.

## Images, charts, and raw data

Prefer Markdown tables for small exact comparisons. Use a static SVG or PNG when
a relationship is clearer visually. Every image needs descriptive alt text and
must remain legible in the light and dark Material palettes.

Validation charts live under each locale's `assets/validation` tree. The
underlying JSON and CSV, publication manifest, and chart-generation identity are
published beside them. Both language trees carry identical chart and raw-data
bytes; captions and surrounding analysis are translated in Markdown.

Do not edit generated chart pixels by hand. Rebuild them from frozen data and
run:

```text
python tools/validate_m5_1_validation_assets.py
```

## Current and future features

Document only commands and formats present in the current binary. The plugin
page may explain the reserved architectural boundary and must state that loading
is not implemented. It does not show a plugin manifest or configuration syntax.

There is no general result export command in `0.1.0-alpha.1`. Result pages use
`result inspect`, `result verify`, `result trajectory`, `run report`, and the
advanced read-only SQLite schema. Future product ideas remain in plans until an
implemented interface and tests exist.

## SEO and versioned URLs

Each page needs a distinct title and description. MkDocs Material emits the
canonical link, while the bilingual plugin emits `hreflang="en"` and
`hreflang="zh-CN"`.

The English home page naturally introduces these search topics:

- Lagrangian water-vapor tracking;
- Lagrangian moisture tracking;
- domain filling;
- atmospheric trajectories;
- forward and backward trajectories.

Release documentation is crawlable under an immutable version path. The `dev`
site uses `noindex,nofollow`; root `robots.txt` also disallows `/trajecta/dev/`.
The generated sitemap points crawlers to release URLs.

## Local documentation checks

Create the CLI input and run source checks:

```text
cargo build --locked --package trajecta-cli
python tools/generate_m5_1_reference.py --binary target/debug/trajecta-cli --check
python tools/validate_m5_1_validation_assets.py
python tools/validate_m5_1_docs.py --binary target/debug/trajecta-cli
git diff --check
```

Build and inspect the release site:

```text
mkdocs build --strict
python tools/validate_m5_1_docs.py \
  --binary target/debug/trajecta-cli --site-dir site
```

Then build the dev variant:

```text
mkdocs build --strict -f mkdocs.dev.yml -d site-dev
python tools/validate_m5_1_docs.py \
  --binary target/debug/trajecta-cli \
  --site-dir site-dev --expect-noindex
```

The source validator checks:

| Check | Reason |
| --- | --- |
| Language file and heading parity | Keep navigation and section links aligned |
| Navigation equals published Markdown set | Prevent orphan and missing pages |
| Front matter | Supply page titles and search descriptions |
| Local links and example snippets | Catch moved pages and stale project copies |
| Documented CLI paths | Reject commands absent from the frozen command tree |
| Generated reference drift | Keep binary help, contract, schemas, and source diagnostics synchronized |
| Style and terminology scope | Keep Chinese prose readable and comparison terms in Validation |
| Built canonical, hreflang, and noindex tags | Check release and dev publication behavior |

Use `mkdocs serve` for visual review. Check both languages, narrow and wide
viewport widths, code wrapping, tables, admonitions, chart labels, and the
language/version switch.

## Adding or moving a page

1. Choose the section according to the reader question.
2. Add the same relative file under `en` and `zh-CN`.
3. Write unique front matter and matching heading levels.
4. Add one navigation entry to `mkdocs.yml`.
5. Update incoming links if a page moved; leave no orphan Markdown file.
6. Add an executable example or source link for commands and formats.
7. Run source validation, strict release/dev builds, and visual review.
8. Include the page in a packaged quickstart only when it is needed offline.

Ordinary documentation commits update `dev`. A release snapshot is created by
the publication workflow after the version and release event have been selected.

## Secret and path review

Documentation and examples are public source. Before committing, scan changed
files for API keys, bearer tokens, private URLs, local usernames, home-directory
paths, and provider credential files. Use placeholders such as `<project>` and
document the official credential channel without copying a real value.

Generated sites and uploaded CI artifacts deserve the same review. A value
excluded from Markdown can still appear in a captured transcript or helper log
if a test prints its environment.
