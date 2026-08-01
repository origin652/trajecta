---
title: Documentation contributions
description: Source layout, bilingual policy, executable snippets, generated reference, style, SEO, and local checks for Trajecta docs.
---

# Documentation contributions

User documentation lives under `docs/user`. English is the normative source;
Chinese pages use the same relative paths and heading structure. Engineering
plans and delivery reports live under `docs/engineering` and are excluded from
the site navigation, search, and sitemap.

## Content rules

- Write Markdown and use MkDocs Material components already enabled in
  `mkdocs.yml`.
- Keep example projects under `examples/` as the configuration truth. Include
  snippets from those files instead of copying YAML into a second source.
- Regenerate CLI, schema, and diagnostic pages with
  `tools/generate_m5_1_reference.py`.
- Describe only implemented commands and platform claims backed by a gate.
- Keep future plugin and export ideas explicitly unavailable.
- Add a distinct title and description to every published page.
- Place comparison terminology and related charts only inside Validation.

Chinese prose uses an academic manual tone. Define assumptions and scope before
procedures. Prefer tables or lists when a sentence contains several independent
conditions.

## Local checks

```text
python tools/generate_m5_1_reference.py --binary target/debug/trajecta-cli --check
python tools/validate_m5_1_docs.py
mkdocs build --strict
```

On Windows, use `target/debug/trajecta-cli.exe`. External links are checked in CI
with retries; a local strict build does not require provider credentials.
