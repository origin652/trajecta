---
title: Use an AI companion with Trajecta configuration
description: Give an AI assistant a bounded Trajecta configuration task, keep one source of truth, and validate every proposed change locally.
---

# AI-assisted configuration

An AI assistant can help select fields, explain diagnostics, and propose a
sequence of `config set` or `project set` commands. Trajecta's local files and
validators remain the configuration interface, so the same Case, Profile, and
project index are reviewed whether changes are typed by a person or proposed by
an assistant.

This page describes a working convention. Trajecta `0.1.0-alpha.1` does not
include an AI service, network connector, or plugin runtime.

## Choose a bounded task

Give the assistant one outcome at a time. Useful examples include:

- add a RunProfile for an existing Case and dataset mapping;
- change the simulation interval while preserving output cadence;
- interpret one `project validate` diagnostic;
- prepare a data-plan review checklist;
- compare resolved forward and backward Profiles;
- estimate a scheduler memory request from a known particle count.

A request such as “configure my whole experiment” leaves scientific choices
implicit. State the quantities that come from the study design: direction,
period, domain, population, release or filling rule, output interval, dataset
family, and available local resources.

## Share the configuration sources

For a project task, the smallest useful bundle is usually:

| File or output | Why it is useful |
| --- | --- |
| `trajecta-project.yml` | Names the Cases, Profiles, datasets, and locks |
| Selected Case | Defines time, domain, population, and scientific intent |
| Selected RunProfile | Defines execution and dataset bindings |
| `project show` output | Shows derived project state and resolved index view |
| `project data-plan` output | Shows requested coverage and capabilities |
| Relevant JSON Schema | Provides exact field names, types, and allowed values |
| One structured diagnostic | Identifies the failed command and stable code |

Share only the files needed for that task. Provider credentials, private keys,
access tokens, and unrelated absolute paths are outside the configuration
question and can stay in their normal local stores.

The public schemas are under `testdata/` in the source repository and are also
listed in the [schema reference](../reference/schemas.md). CLI syntax comes from
`trajecta --help` and the [command reference](../reference/cli.md).

## Ask for commands rather than a second document

Have the assistant return a short explanation followed by exact commands. For
example:

```text
I am editing the Trajecta project at PROJECT.
Use only commands available in 0.1.0-alpha.1.
The selected Profile is PROFILE and it uses CASE.
Propose the smallest sequence of project get/set/unset commands needed to
change WORKER_THREADS to 4. Do not rewrite unrelated fields.
Finish with read-only validation commands. Do not run any command.
```

This format keeps the project file as the single source of truth. It also makes
each proposed change visible in shell history and easy to review before
execution.

For a new structured document, ask the assistant to start from a repository
example and return a patch against that file. Include the matching schema and
the exact Trajecta release in the request.

## Inspect before applying

Read the existing value and project state:

```text
trajecta --project PROJECT project get profile.PROFILE
trajecta --project PROJECT project status
```

Review the proposed selector, value type, units, and path. Fields with similar
names may use different units; for example, execution memory is expressed in
bytes, while the machine scheduler pool uses MiB.

Run one change at a time:

```text
trajecta --project PROJECT project set SELECTOR VALUE
trajecta --project PROJECT project show
```

Trajecta validates an updated document before replacing the old file. A failed
edit can be returned to the assistant together with its exact diagnostic code
and the relevant schema fragment.

## Close the local validation loop

After the last edit, run the checks that match the affected layer:

```text
trajecta config validate
trajecta --project PROJECT project validate
trajecta case resolve PROJECT/cases/CASE.yml
trajecta --project PROJECT project data-plan --output PROJECT/data-plan.json
trajecta --project PROJECT doctor --deep
```

Read the resolved Case and data plan directly. Confirm the physical interval,
domain, direction, population size or filling rule, output cadence, dataset
profile, coverage anchors, and local target roots.

When the project has data and locks, complete the check with:

```text
trajecta --project PROJECT project finalize
trajecta --project PROJECT project status
```

The next run then consumes the same finalized documents that were reviewed.

## Use structured diagnostics for another turn

JSON output gives the assistant a concise failure record:

```text
trajecta --format json --project PROJECT project validate
trajecta --format json --project PROJECT doctor --deep
```

Send the `command`, diagnostic `code`, `message`, and `hint`, together with the
smallest relevant document excerpt. Stable codes are preferable to a screenshot
of terminal formatting because they map directly to the
[diagnostic reference](../reference/diagnostics.md).

A useful follow-up request is:

```text
Explain this diagnostic in the context of the supplied schema. Identify the
smallest selector change that can resolve it. Preserve all scientific choices
that are not named in the diagnostic. Return commands for review, followed by
the validation commands.
```

## Review scientific choices separately

Schema validation establishes that a document is well formed and internally
coherent. Study-specific decisions still benefit from an explicit review. A
compact table keeps that review focused:

| Choice | Value to confirm |
| --- | --- |
| Direction | Forward or backward |
| Physical interval | Start, end, and integration step |
| Output | Initial/final coverage and scheduled interval |
| Population | Domain fill, release, air mass, or ozone setup |
| Domain | Horizontal boundary and vertical limits |
| Meteorology | Dataset family, vertical coordinate, and reader |
| Resources | Worker threads and memory budget |

Add the final table to the project's research notes when those decisions need a
human record. Trajecta stores the resolved machine inputs in each run directory
for later inspection.

## Work iteratively when data is absent

AI-assisted configuration can stop at the `configured` project state. Generate
and review `data-plan.json`, then prepare meteorological files through the
[deferred-data workflow](deferred-data.md). After the files arrive, finalize the
same project rather than asking the assistant to invent a DatasetLock.

DatasetLocks are produced from inspected local files. Their content includes
file identities and capabilities that are available only after acquisition and
preparation.
