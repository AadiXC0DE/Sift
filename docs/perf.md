# Perf (Section 15)

| Metric | Budget | Last (M1 Air, debug) | Date |
|---|---|---|---|
| Cold start warm -> rows | <=400ms | 340ms p95 (bench-start) | 2026-09-04 |
| j/k focus | <=16ms | <16ms (event-only, no IPC) | 2026-09-04 |
| Open thread LRU | <=16ms | swap prerendered iframe | 2026-09-04 |
| Archive row gone | <=30ms | optimistic <5ms | 2026-09-04 |
| Search keystroke 100k | <=30ms | FTS prefix | 2026-09-04 |
| threads_query 100 rows | <=3ms | covering index | 2026-09-04 |

Release-build numbers recorded at RC.
