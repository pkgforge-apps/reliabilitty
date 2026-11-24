# Performance

A simple benchmark of Reliabilitty against st, xfce4-terminal, Ptyxis,
Alacritty, Ghostty and foot. The numbers are indicative, not a full benchmark
suite.

## Environment

- Artix Linux, kernel 7.3.4-artix1-2, X11 with i3 and a compositor.
- CPU: Intel Xeon E5-1650 v4 @ 3.60 GHz.
- GPU: AMD Radeon RX 580 2048SP.
- Real desktop session, one terminal running at a time.
- Clean `$XDG_CONFIG_HOME` under `/tmp` for every terminal. xfce4-terminal and
  Ptyxis also get a private D-Bus session so their real settings cannot leak in
  through xfconf or GSettings. Ghostty behaves very differently with the real
  config; see "Ghostty and GTK4" below.
- foot is Wayland only and was run on the compositor at
  [12to11](https://github.com/fish4terrisa-MSDSM/12to11). Cross-checks with
  Alacritty on both 12to11 and plain X11 showed no significant difference in
  any metric, so the Wayland results are comparable to the X11 ones.
- Same machine for every run.

## Versions

| Terminal | Version | Renderer |
|---|---|---|
| Reliabilitty | 0.1.20 | CPU, swash |
| Alacritty | 0.17.0 | GPU, OpenGL |
| foot | 1.28.0 | CPU, fcft |
| Ghostty | 1.4.0-arch1 | GPU, OpenGL |
| Ptyxis | 50.1 | GPU, GTK4/VTE |
| st | 0.9.3 | CPU, Xft |
| xfce4-terminal | 1.2.0 | CPU, VTE |

## Method

- Launch time: process spawn until the active window class matches, the same
  method as the `bench-startup` helper. Median of 5 runs.
- RSS and PSS: sum of the process tree after one second idle. Median of 5
  runs.
- Draw time: spawn until a marker written by the child after printing 10000
  lines and hiding the cursor appears. This measures how fast the terminal
  processes the output through the PTY. Median of 10 runs.
- CPU: task-clock from `perf stat` over the draw window, in milliseconds. This
  is exact CPU time with no kernel-tick quantisation; the value covers the
  whole process tree including startup. Median of 5 runs. For foot under 12to11
  this excludes the compositor's rendering work, so its figure is partial.

## Results

### Launch time

| # | Terminal | Launch time |
|--:|---|---:|
| 1 | Reliabilitty | 11.8 ms |
| 2 | foot | 23.2 ms |
| 3 | st | 41.5 ms |
| 4 | Alacritty | 116.0 ms |
| 5 | xfce4-terminal | 154.4 ms |
| 6 | Ghostty | 160.0 ms |
| 7 | Ptyxis | 280.9 ms |

### RSS

| # | Terminal | RSS |
|--:|---|---:|
| 1 | Reliabilitty | 14.7 MiB |
| 2 | st | 15.3 MiB |
| 3 | foot | 23.5 MiB |
| 4 | xfce4-terminal | 47.0 MiB |
| 5 | Alacritty | 58.0 MiB |
| 6 | Ptyxis | 101.3 MiB |
| 7 | Ghostty | 114.0 MiB |

### PSS

| # | Terminal | PSS |
|--:|---|---:|
| 1 | st | 4.4 MiB |
| 2 | foot | 9.8 MiB |
| 3 | Reliabilitty | 10.0 MiB |
| 4 | xfce4-terminal | 17.7 MiB |
| 5 | Alacritty | 32.9 MiB |
| 6 | Ptyxis | 56.5 MiB |
| 7 | Ghostty | 78.7 MiB |

### 10000 lines

| # | Terminal | Draw time |
|--:|---|---:|
| 1 | Reliabilitty | 16.1 ms |
| 2 | foot | 41.2 ms |
| 3 | st | 44.0 ms |
| 4 | Alacritty | 116.5 ms |
| 5 | xfce4-terminal | 128.7 ms |
| 6 | Ghostty | 237.5 ms |
| 7 | Ptyxis | 267.4 ms |

### CPU while drawing 10000 lines

| # | Terminal | CPU time (perf task-clock) |
|--:|---|---:|
| 1 | Reliabilitty | 43.7 ms |
| 2 | st | 49.5 ms |
| 3 | Ptyxis | 61.0 ms |
| 4 | xfce4-terminal | 64.9 ms |
| 5 | foot | 75.1 ms |
| 6 | Alacritty | 146.2 ms |
| 7 | Ghostty | 390.7 ms |

Reliabilitty and st are effectively tied on CPU time; both use a fraction of
what the GPU terminals consume. foot's figure is a partial count because its
rendering runs in the 12to11 compositor process.

## Combined

| Terminal | Launch | RSS | PSS | Draw | CPU |
|---|---:|---:|---:|---:|---:|
| Reliabilitty | 11.8 ms | 14.7 MiB | 10.0 MiB | 16.1 ms | 43.7 ms |
| Alacritty | 116.0 ms | 58.0 MiB | 32.9 MiB | 116.5 ms | 146.2 ms |
| Ghostty | 160.0 ms | 114.0 MiB | 78.7 MiB | 237.5 ms | 390.7 ms |
| Ptyxis | 280.9 ms | 101.3 MiB | 56.5 MiB | 267.4 ms | 61.0 ms |
| st | 41.5 ms | 15.3 MiB | 4.4 MiB | 44.0 ms | 49.5 ms |
| xfce4-terminal | 154.4 ms | 47.0 MiB | 17.7 MiB | 128.7 ms | 64.9 ms |
| foot | 23.2 ms | 23.5 MiB | 9.8 MiB | 41.2 ms | 75.1 ms |

Reliabilitty is fastest to launch, draws text fastest, uses the least CPU and
now has the lowest RSS of the group. st keeps the lowest PSS because it is a
smaller C process. The GPU terminals are several times slower on launch, draw
and CPU.

Earlier releases used mimalloc as the global allocator, which cost roughly
twice the RSS (about 14 -> 27 MiB). It was dropped again in v0.1.20 after lazy
font loading and chunked scrollback removed the allocation pressure that had
made mimalloc worthwhile; CPU and draw time still improved in that release.

The GPU terminals are now level on raw PTY read throughput: Alacritty's
vtebench total is 662 ms versus Reliabilitty's 661 ms. On launch, draw time and
CPU use they are several times slower. That does not make GPU rendering useless
in every case, but for a normal terminal workload the CPU path is already cheap
enough that the extra machinery buys very little.

## vtebench

[vtebench](https://github.com/alacritty/vtebench) measures how long a child
blocks writing to the PTY while the terminal reads and processes the output.
It stresses PTY read throughput only, not frame rate or latency, so these
numbers are separate from the draw and CPU results above. Each terminal ran
the full suite with the default settings (1 MiB minimum sample, 10 seconds per
benchmark). Values are mean milliseconds; lower is better.

| Benchmark | Reliabilitty | Alacritty | foot | Ghostty | Ptyxis | st | xfce4-terminal |
|---|---:|---:|---:|---:|---:|---:|---:|
| cursor_motion | 9 ms | 12 ms | 7 ms | 9 ms | 22 ms | 52 ms | 17 ms |
| dense_cells | 19 ms | 33 ms | 70 ms | 34 ms | 135 ms | 931 ms | 162 ms |
| light_cells | 3 ms | 10 ms | 8 ms | 2 ms | 6 ms | 25 ms | 8 ms |
| medium_cells | 6 ms | 15 ms | 16 ms | 10 ms | 18 ms | 34 ms | 20 ms |
| scrolling | 119 ms | 111 ms | 128 ms | 129 ms | 216 ms | 376 ms | 207 ms |
| scrolling_bottom_region | 123 ms | 109 ms | 124 ms | 120 ms | 189 ms | 380 ms | 201 ms |
| scrolling_bottom_small_region | 120 ms | 112 ms | 127 ms | 121 ms | 201 ms | 342 ms | 209 ms |
| scrolling_fullscreen | 5 ms | 12 ms | 7 ms | 27 ms | 15 ms | 27 ms | 16 ms |
| scrolling_top_region | 122 ms | 112 ms | 127 ms | 120 ms | 229 ms | 325 ms | 241 ms |
| scrolling_top_small_region | 122 ms | 111 ms | 129 ms | 127 ms | 199 ms | 343 ms | 222 ms |
| sync_medium_cells | 6 ms | 16 ms | 14 ms | 10 ms | 19 ms | 36 ms | 21 ms |
| unicode | 7 ms | 9 ms | 25 ms | 3 ms | 36 ms | 156 ms | 76 ms |
| **Total** | **661 ms** | **662 ms** | **782 ms** | **712 ms** | **1285 ms** | **3027 ms** | **1400 ms** |

Reliabilitty now ties Alacritty on pure PTY read throughput: 661 ms against
662 ms, inside the run-to-run spread. Reliabilitty is about 4.5x faster than st
overall, ahead of both VTE terminals and foot, and leads the GPU terminals
decisively on launch time, draw time and CPU use.

## Frame rate (DOOM-fire)

[DOOM-fire](https://github.com/const-void/DOOM-fire-zig) animates a fire in a
grid of colored blocks and reports its own frame rate. That number counts the
frames it writes to the PTY before the terminal reads them, so it measures PTY
throughput under backpressure, not frames actually presented on screen. The
tool's own README calls the figure unscientific. It is included here because
issue #12 asked for frame rate to be measured alongside latency.

Every terminal was forced to the same 1440x660 window with i3 (floating) and
its font tuned so it rendered close to 160x30 cells. The actual grids were
118-160 columns by 30-40 rows, but every run had roughly 4700-4900 cells, within
a few percent of the 4800 cells in a 160x30 grid, so both the pixel area and the
bytes per frame were comparable across terminals. Transparency was disabled and
each terminal got a clean config with a private D-Bus session. The relay drove
DOOM-fire for 8 or 12 seconds and reported the median FPS it wrote to the PTY.

To measure frames actually presented on screen, the window was recorded with
`ffmpeg -f x11grab -framerate 60 -f framemd5` and consecutive distinct frame
hashes were counted. The 60 Hz capture is a ceiling on these numbers. Presented
FPS below is the steady rate once the fire is running; the average over the
whole capture is lower because it includes startup and teardown.

| Terminal | Written to PTY | Presented, steady | Presented, peak |
|---|---:|---:|---:|
| Alacritty | 937 | 50 | 60 |
| Reliabilitty | 1095 | 49 | 60 |
| foot | 986 | 46 | 60 |
| Ghostty | 845 | 45 | 57 |
| Ptyxis | 310 | 42 | 59 |
| xfce4-terminal | 236 | 19 | 23 |
| st | 137 | 13 | 22 |

The two columns measure different things. Written-to-PTY is DOOM-fire's own
reported FPS: how fast it can push frames into the terminal. Presented FPS is
how often the terminal actually repainted. Reliabilitty writes the most frames
and presents about as many as Alacritty, because its renderer coalesces bursts
into one paint per display frame. The frame interval is derived from the
display's refresh rate through RandR, so rtty paints at most once per frame
instead of wasting work on frames the compositor would never show.

foot runs under 12to11, so its compositor's rendering work is not counted. The
draw, CPU and vtebench results above remain the better evidence for the speed
claims. DOOM-fire's `mem:` line is the per-frame output buffer size (about 16
KiB minimum, 96 KiB average and 100 KiB maximum in these runs), not process RAM.

## Ghostty and GTK4

The tables above use a clean config so every terminal starts on the same
footing. With the real config, Ghostty alone takes about 1.7 s to launch
versus ~180 ms clean; the others do not use GTK4 (st: Xlib, Alacritty:
winit/glutin, Reliabilitty: x11rb, xfce4-terminal: GTK3, foot: fcft/Wayland)
and are unaffected.

The cause is a GTK4 bug, not Ghostty: `GApplication::startup` does a themed
window-icon lookup that loads the whole icon theme, and its cache parser is
quadratic. The active theme, `Papirus-Dark`, has roughly 102 sub-directories
and 297,000 cache entries, so startup does about 30 million inspections and
loads the theme twice. See
[GTK4-slow-launch-speed-test](https://github.com/Samueru-sama/GTK4-slow-launch-speed-test)
for the investigation and a minimal reproduction.
