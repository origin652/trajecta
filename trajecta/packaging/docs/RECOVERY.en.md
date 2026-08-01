# Emergency recovery

1. Preserve the run directory, job database, SQLite sidecars, and forensic files.
2. Run `trajecta job status JOB_ID` and `trajecta result inspect RESULT`.
3. Run `trajecta doctor --deep` before admitting new work.
4. Use `job rerun` only after the original attempt reaches a terminal state.
5. `job prune` is dry-run only in this release; it does not delete artifacts.

Do not manually remove a WAL file or rewrite a run manifest.
