# Ignored diagnostic ablation

All four exact tests ran against one isolated synthetic flat directory with 16 Markdown files. HOME/profile configuration was redirected to an isolated diagnostic home. The files contain only generated test text. Every command exited 0 and ran one explicitly selected ignored test. These measurements compare observation contracts, not speed.

## Probe 1

```text
target/release/deps/open_folder_cost-6581ecae744a7d3e.exe --ignored --exact attribute_the_cost_of_opening_a_folder --nocapture
```

```text

running 1 test
test attribute_the_cost_of_opening_a_folder ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.04s

measuring Q:\repos\markturbo\.scratch\test-ablation\diagnostic-input
     393µs  read_dir (depth 0)  16 entries
     184µs  read_dir (depth 1)  16 entries, 0 children
     784µs  Watcher::new (recursive)  ok
       1ms  skill::discover (workspace)  0 skills
       3ms  skill cold 1  0 skills
       3ms  skill cold 2  0 skills
       3ms  skill cold 3  0 skills
       4ms  skill warm 1  0 skills
       4ms  skill warm 2  0 skills
       4ms  skill warm 3  0 skills
       4ms  skill warm 4  0 skills
       4ms  skill warm 5  0 skills
       3ms  instruction::discover  0 files
       7ms  instruction::discover_with (global)  0 files
```

## Probe 2

```text
target/release/deps/open_folder_cost-6581ecae744a7d3e.exe --ignored --exact read_dir_on_a_flat_directory --nocapture
```

```text

running 1 test
test read_dir_on_a_flat_directory ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s

     208µs  flat read_dir  16 entries
```

## Probe 3

```text
target/release/deps/search_cost-78294e9cf8ae9e92.exe --ignored --exact attribute_the_cost_of_a_folder_search --nocapture
```

```text

running 1 test
walk:   330.7µs  (16 documents)
query                  the:    2.9ms  500 match(es) in 10 file(s) [capped]
query            workspace:    4.0ms  500 match(es) in 10 file(s) [capped]
query   zzzz-no-such-token:    5.9ms  0 match(es) in 0 file(s)
test attribute_the_cost_of_a_folder_search ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.01s

```

## Probe 4

```text
target/release/deps/search_cost-78294e9cf8ae9e92.exe --ignored --exact the_cap_bounds_the_filesystem_work_not_only_the_list --nocapture
```

```text

running 1 test
                 the: walk 276.5µs  total   2.8ms  500 match(es) [capped]
  zzzz-no-such-token: walk 178.7µs  total   4.7ms  0 match(es)
test the_cap_bounds_the_filesystem_work_not_only_the_list ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.01s

```

## Decisions

- Remove `read_dir_on_a_flat_directory`: it calls the same `workspace::read_dir` on the same `bench_dir` and exposes the same count and elapsed read as the retained phase diagnostic's depth-zero row. Both reported 16 entries.
- Retain `the_cap_bounds_the_filesystem_work_not_only_the_list`: both search probes reported 500 capped common matches and zero uncapped rare matches, but this probe re-enumerates for each query and measures combined walk-plus-search elapsed time. The general probe walks once and measures queries against that shared inventory, so the measurement contracts are not identical.
- Do not compare the numerical timings as a performance improvement. No DPAPI, desktop, or native GUI acceptance is inferred.
