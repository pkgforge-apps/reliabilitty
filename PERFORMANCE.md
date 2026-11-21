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
| Reliabilitty | 0.1.17 | CPU, swash |
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
| 1 | Reliabilitty | 16.0 ms |
| 2 | foot | 25.4 ms |
| 3 | st | 43.1 ms |
| 4 | Alacritty | 126.5 ms |
| 5 | xfce4-terminal | 166.0 ms |
| 6 | Ghostty | 193.9 ms |
| 7 | Ptyxis | 324.2 ms |

### RSS

| # | Terminal | RSS |
|--:|---|---:|
| 1 | st | 15.3 MiB |
| 2 | foot | 23.6 MiB |
| 3 | Reliabilitty | 26.9 MiB |
| 4 | xfce4-terminal | 79.8 MiB |
| 5 | Alacritty | 95.9 MiB |
| 6 | Ptyxis | 138.6 MiB |
| 7 | Ghostty | 152.0 MiB |

### PSS

| # | Terminal | PSS |
|--:|---|---:|
| 1 | st | 4.5 MiB |
| 2 | foot | 9.9 MiB |
| 3 | Reliabilitty | 22.3 MiB |
| 4 | xfce4-terminal | 30.5 MiB |
| 5 | Alacritty | 44.7 MiB |
| 6 | Ptyxis | 68.2 MiB |
| 7 | Ghostty | 90.6 MiB |

### 10000 lines

| # | Terminal | Draw time |
|--:|---|---:|
| 1 | Reliabilitty | 28.6 ms |
| 2 | foot | 40.1 ms |
| 3 | st | 48.4 ms |
| 4 | Alacritty | 130.0 ms |
| 5 | xfce4-terminal | 161.3 ms |
| 6 | Ghostty | 265.2 ms |
| 7 | Ptyxis | 311.2 ms |

### CPU while drawing 10000 lines

| # | Terminal | CPU time (perf task-clock) |
|--:|---|---:|
| 1 | Reliabilitty | 50.4 ms |
| 2 | st | 49.8 ms |
| 3 | xfce4-terminal | 62.7 ms |
| 4 | foot | 88.3 ms |
| 5 | Ptyxis | 96.3 ms |
| 6 | Alacritty | 164.6 ms |
| 7 | Ghostty | 450.4 ms |

Reliabilitty and st are effectively tied on CPU time; both use a fraction of
what the GPU terminals consume. foot's figure is a partial count because its
rendering runs in the 12to11 compositor process.

## Combined

| Terminal | Launch | RSS | PSS | Draw | CPU |
|---|---:|---:|---:|---:|---:|
| Reliabilitty | 16.0 ms | 26.9 MiB | 22.3 MiB | 28.6 ms | 50.4 ms |
| Alacritty | 126.5 ms | 95.9 MiB | 44.7 MiB | 130.0 ms | 164.6 ms |
| Ghostty | 193.9 ms | 152.0 MiB | 90.6 MiB | 265.2 ms | 450.4 ms |
| Ptyxis | 324.2 ms | 138.6 MiB | 68.2 MiB | 311.2 ms | 96.3 ms |
| st | 43.1 ms | 15.3 MiB | 4.5 MiB | 48.4 ms | 49.8 ms |
| xfce4-terminal | 166.0 ms | 79.8 MiB | 30.5 MiB | 161.3 ms | 62.7 ms |
| foot | 25.4 ms | 23.6 MiB | 9.9 MiB | 40.1 ms | 88.3 ms |

Reliabilitty is fastest to launch, draws text fastest, and is tied with st for
the least CPU use. st wins RSS and PSS because it is a smaller C process. The
GPU terminals are several times slower on launch, draw and CPU.

Reliabilitty uses about twice the memory it did before switching to mimalloc
as the global allocator (roughly 14 -> 27 MiB RSS). mimalloc reserves and
commits arena memory up front; this is the cost of the allocator change and is
why st and foot have an edge on RSS and PSS. mimalloc was briefly removed in
v0.1.13, which cut RSS but regressed draw time and CPU, so it was restored in
v0.1.14.

The GPU terminals remain slightly ahead only on raw PTY read throughput:
Alacritty's vtebench total is 660 ms versus Reliabilitty's 711 ms. On launch,
draw time and CPU use they are several times slower. That does not make GPU
rendering useless in every case, but for a normal terminal workload the CPU
path is already cheap enough that the extra machinery buys very little.

## vtebench

[vtebench](https://github.com/alacritty/vtebench) measures how long a child
blocks writing to the PTY while the terminal reads and processes the output.
It stresses PTY read throughput only, not frame rate or latency, so these
numbers are separate from the draw and CPU results above. Each terminal ran
the full suite with the default settings (1 MiB minimum sample, 10 seconds per
benchmark). Values are mean milliseconds; lower is better.

| Benchmark | Reliabilitty | Alacritty | foot | Ghostty | Ptyxis | st | xfce4-terminal |
|---|---:|---:|---:|---:|---:|---:|---:|
| cursor_motion | 14 ms | 12 ms | 7 ms | 9 ms | 20 ms | 51 ms | 18 ms |
| dense_cells | 53 ms | 32 ms | 65 ms | 32 ms | 86 ms | 698 ms | 93 ms |
| light_cells | 7 ms | 10 ms | 8 ms | 2 ms | 6 ms | 25 ms | 6 ms |
| medium_cells | 8 ms | 11 ms | 16 ms | 10 ms | 17 ms | 31 ms | 16 ms |
| scrolling | 122 ms | 112 ms | 130 ms | 132 ms | 196 ms | 389 ms | 186 ms |
| scrolling_bottom_region | 121 ms | 110 ms | 126 ms | 123 ms | 179 ms | 386 ms | 179 ms |
| scrolling_bottom_small_region | 118 ms | 113 ms | 127 ms | 122 ms | 191 ms | 341 ms | 186 ms |
| scrolling_fullscreen | 7 ms | 11 ms | 7 ms | 27 ms | 13 ms | 27 ms | 14 ms |
| scrolling_top_region | 118 ms | 114 ms | 125 ms | 125 ms | 225 ms | 338 ms | 216 ms |
| scrolling_top_small_region | 120 ms | 113 ms | 126 ms | 121 ms | 200 ms | 345 ms | 201 ms |
| sync_medium_cells | 8 ms | 13 ms | 14 ms | 10 ms | 17 ms | 34 ms | 17 ms |
| unicode | 15 ms | 9 ms | 24 ms | 3 ms | 33 ms | 165 ms | 49 ms |
| **Total** | **711 ms** | **660 ms** | **775 ms** | **716 ms** | **1183 ms** | **2830 ms** | **1181 ms** |

Alacritty leads on pure PTY read throughput, but only by a narrow margin:
660 ms in total versus Reliabilitty's 711 ms. The gap is inside the run-to-run
spread. Reliabilitty is about 4x faster than st overall, ahead of both VTE
terminals and foot, and leads the GPU terminals decisively on launch time, draw
time and CPU use.

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
| Alacritty | 979 | 50 | 60 |
| foot | 983 | 49 | 60 |
| Ghostty | 856 | 49 | 60 |
| Reliabilitty | 1148 | 48 | 60 |
| Ptyxis | 329 | 34 | 42 |
| xfce4-terminal | 281 | 20 | 26 |
| st | 207 | 14 | 23 |

The two columns measure different things. Written-to-PTY is DOOM-fire's own
reported FPS: how fast it can push frames into the terminal. Presented FPS is
how often the terminal actually repainted. Reliabilitty writes far more frames
than any other terminal but presents about as many as the GPU terminals,
because its renderer coalesces bursts into one paint per display frame. The
frame interval is derived from the display's refresh rate through RandR, so
rtty paints at most once per frame instead of wasting work on frames the
compositor would never show.

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
