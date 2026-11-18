A call made inside a call (Orbit's H3 segment, under `render_shot`) shows in `cozy run watch --full` and the page's report with its
own stages, steps and memory, at its place (`0/0`). Its machine journaled it only on its parent call's log, which no one reads, so
the run showed its whole GPU time as unaccounted.
