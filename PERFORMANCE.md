# Performance

A simple benchmark of Reliabilitty against st, xfce4-terminal, Ptyxis,
Alacritty, Ghostty, foot and Konsole. The numbers are indicative, not a full
benchmark suite.

## Environment

- Artix Linux, kernel 7.3.4-artix1-2, X11 with i3 and a compositor.
- CPU: Intel Xeon E5-1650 v4 @ 3.60 GHz.
- GPU: AMD Radeon RX 580 2048SP.
- Real desktop session, one terminal running at a time.
- Clean `$XDG_CONFIG_HOME` under `/tmp` for every terminal. xfce4-terminal,
  Ptyxis and Konsole also get a private D-Bus session so their real settings
  cannot leak in through xfconf, GSettings or KConfig. Ghostty behaves very
  differently with the real config; see "Ghostty and GTK4" below.
- foot is Wayland only and was run on the compositor at
  [12to11](https://github.com/fish4terrisa-MSDSM/12to11). Cross-checks with
  Alacritty on both 12to11 and plain X11 showed no significant difference in
  any metric, so the Wayland results are comparable to the X11 ones.
- Same machine for every run.

## Versions

| Terminal | Version | Renderer |
|---|---|---|
| Reliabilitty | 0.1.21 | CPU, swash |
| Alacritty | 0.17.0 | GPU, OpenGL |
| foot | 1.28.0 | CPU, fcft |
| Ghostty | 1.4.0-arch2 | GPU, OpenGL |
| konsole | 26.08.3 | CPU, QPainter |
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
- CPU: task-clock from `perf stat` from spawn until that same marker appears,
  in milliseconds. This is exact CPU time with no kernel-tick quantisation; the
  value covers the whole process tree including startup. Median of 5 runs. For
  foot under 12to11 this excludes the compositor's rendering work, so its
  figure is partial.

## Results

### Launch time

| # | Terminal | Launch time |
|--:|---|---:|
| 1 | Reliabilitty | 12.2 ms |
| 2 | foot | 23.0 ms |
| 3 | st | 45.7 ms |
| 4 | Alacritty | 106.1 ms |
| 5 | xfce4-terminal | 139.9 ms |
| 6 | Ghostty | 163.6 ms |
| 7 | konsole | 231.5 ms |
| 8 | Ptyxis | 284.5 ms |

### RSS

| # | Terminal | RSS |
|--:|---|---:|
| 1 | Reliabilitty | 14.7 MiB |
| 2 | st | 15.4 MiB |
| 3 | foot | 23.5 MiB |
| 4 | xfce4-terminal | 47.1 MiB |
| 5 | Alacritty | 58.5 MiB |
| 6 | Ptyxis | 101.9 MiB |
| 7 | konsole | 105.6 MiB |
| 8 | Ghostty | 114.4 MiB |

### PSS

| # | Terminal | PSS |
|--:|---|---:|
| 1 | st | 5.0 MiB |
| 2 | Reliabilitty | 10.0 MiB |
| 3 | foot | 10.4 MiB |
| 4 | xfce4-terminal | 18.2 MiB |
| 5 | Alacritty | 35.7 MiB |
| 6 | Ptyxis | 59.9 MiB |
| 7 | konsole | 74.3 MiB |
| 8 | Ghostty | 81.5 MiB |

### 10000 lines

| # | Terminal | Draw time |
|--:|---|---:|
| 1 | Reliabilitty | 14.9 ms |
| 2 | foot | 39.0 ms |
| 3 | st | 44.0 ms |
| 4 | Alacritty | 109.9 ms |
| 5 | xfce4-terminal | 131.9 ms |
| 6 | konsole | 237.4 ms |
| 7 | Ghostty | 245.9 ms |
| 8 | Ptyxis | 272.1 ms |

### CPU while drawing 10000 lines

| # | Terminal | CPU time (perf task-clock) |
|--:|---|---:|
| 1 | Reliabilitty | 32.5 ms |
| 2 | st | 56.9 ms |
| 3 | foot | 67.7 ms |
| 4 | xfce4-terminal | 149.8 ms |
| 5 | Alacritty | 162.4 ms |
| 6 | Ptyxis | 280.4 ms |
| 7 | Ghostty | 300.5 ms |
| 8 | konsole | 531.0 ms |

Reliabilitty uses less CPU than any other terminal, about 40% less than st.
foot's figure is a partial count because its rendering runs in the 12to11
compositor process.

## Combined

| Terminal | Launch | RSS | PSS | Draw | CPU |
|---|---:|---:|---:|---:|---:|
| Reliabilitty | 12.2 ms | 14.7 MiB | 10.0 MiB | 14.9 ms | 32.5 ms |
| Alacritty | 106.1 ms | 58.5 MiB | 35.7 MiB | 109.9 ms | 162.4 ms |
| Ghostty | 163.6 ms | 114.4 MiB | 81.5 MiB | 245.9 ms | 300.5 ms |
| konsole | 231.5 ms | 105.6 MiB | 74.3 MiB | 237.4 ms | 531.0 ms |
| Ptyxis | 284.5 ms | 101.9 MiB | 59.9 MiB | 272.1 ms | 280.4 ms |
| st | 45.7 ms | 15.4 MiB | 5.0 MiB | 44.0 ms | 56.9 ms |
| xfce4-terminal | 139.9 ms | 47.1 MiB | 18.2 MiB | 131.9 ms | 149.8 ms |
| foot | 23.0 ms | 23.5 MiB | 10.4 MiB | 39.0 ms | 67.7 ms |

Reliabilitty is fastest to launch, draws text fastest, uses the least CPU and
has the lowest RSS of the group. st keeps the lowest PSS because it is a smaller
C process. The GPU terminals are several times slower on launch, draw and CPU;
konsole is the slowest of the group on launch, draw and CPU.

Earlier releases used mimalloc as the global allocator, which cost roughly
twice the RSS (about 14 -> 27 MiB). It was dropped again in v0.1.20 after lazy
font loading and chunked scrollback removed the allocation pressure that had
made mimalloc worthwhile; CPU and draw time still improved in that release.

The GPU terminals are level with Reliabilitty on raw PTY read throughput:
Alacritty's vtebench total is 661 ms against Reliabilitty's 649 ms. On launch,
draw time and CPU use the GPU terminals are several times slower. That does not
make GPU rendering useless in every case, but for a normal terminal workload
the CPU path is already cheap enough that the extra machinery buys very little.

## vtebench

[vtebench](https://github.com/alacritty/vtebench) measures how long a child
blocks writing to the PTY while the terminal reads and processes the output.
It stresses PTY read throughput only, not frame rate or latency, so these
numbers are separate from the draw and CPU results above. Each terminal ran
the full suite with the default settings (1 MiB minimum sample, 10 seconds per
benchmark). Values are mean milliseconds; lower is better.

| Benchmark | Reliabilitty | Alacritty | foot | Ghostty | Ptyxis | konsole | st | xfce4-terminal |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| cursor_motion | 8 ms | 12 ms | 6 ms | 9 ms | 22 ms | 32 ms | 51 ms | 18 ms |
| dense_cells | 18 ms | 33 ms | 70 ms | 33 ms | 120 ms | 114 ms | 1002 ms | 115 ms |
| light_cells | 3 ms | 11 ms | 8 ms | 2 ms | 6 ms | 54 ms | 25 ms | 7 ms |
| medium_cells | 6 ms | 15 ms | 17 ms | 10 ms | 18 ms | 183 ms | 34 ms | 18 ms |
| scrolling | 119 ms | 110 ms | 131 ms | 127 ms | 224 ms | 264 ms | 374 ms | 199 ms |
| scrolling_bottom_region | 119 ms | 110 ms | 126 ms | 119 ms | 188 ms | 152 ms | 376 ms | 201 ms |
| scrolling_bottom_small_region | 120 ms | 111 ms | 126 ms | 120 ms | 200 ms | 150 ms | 340 ms | 211 ms |
| scrolling_fullscreen | 5 ms | 12 ms | 7 ms | 27 ms | 17 ms | 49 ms | 27 ms | 16 ms |
| scrolling_top_region | 120 ms | 111 ms | 127 ms | 119 ms | 233 ms | 150 ms | 332 ms | 247 ms |
| scrolling_top_small_region | 118 ms | 110 ms | 130 ms | 122 ms | 207 ms | 146 ms | 336 ms | 228 ms |
| sync_medium_cells | 6 ms | 17 ms | 14 ms | 10 ms | 19 ms | 1352 ms | 35 ms | 22 ms |
| unicode | 7 ms | 9 ms | 25 ms | 3 ms | 41 ms | 79 ms | 133 ms | 83 ms |
| **Total** | **649 ms** | **661 ms** | **787 ms** | **701 ms** | **1295 ms** | **2725 ms** | **3065 ms** | **1365 ms** |

Reliabilitty ties Alacritty on pure PTY read throughput: 649 ms against 661 ms,
inside the run-to-run spread. Reliabilitty is about 4.7x faster than st overall,
ahead of both VTE terminals, foot and Konsole, and leads the GPU terminals
on launch time, draw time and CPU use. konsole is the slowest here by a wide
margin, with sync_medium_cells alone taking 1352 ms.

## Frame rate (DOOM-fire)

[DOOM-fire](https://github.com/const-void/DOOM-fire-zig) animates a fire in a
grid of colored blocks and reports its own frame rate. That number counts the
frames it writes to the PTY before the terminal reads them, so it measures PTY
throughput under backpressure, not frames actually presented on screen. The
tool's own README calls the figure unscientific. It is included here because
issue #12 asked for frame rate to be measured alongside latency.

Every terminal was forced to the same 1440x660 window with i3 (floating) and
its font tuned so it rendered close to 160x30 cells. The actual grids were
118-160 columns by 30-40 rows, around 4700-4900 cells, within a few percent of
the 4800 cells in a 160x30 grid, so both the pixel area and the bytes per frame
were comparable across terminals. Transparency was disabled and each terminal
got a clean config with a private D-Bus session. The relay drove DOOM-fire for
20 seconds and reported the median FPS it wrote to the PTY.

To measure frames actually presented on screen, the window was recorded with
`ffmpeg -f x11grab -framerate 120 -f framemd5` and consecutive distinct frame
hashes were counted. The panel runs at 78.85 Hz, below the 120 fps capture, so
the capture does not limit the result. Presented FPS is the median of the
steady seconds once the fire is fully running; the average over the whole
capture is lower because it includes startup and teardown.

| Terminal | Written to PTY | Presented, steady | Presented, average |
|---|---:|---:|---:|
| Reliabilitty | 1081 | 79.0 | 44.6 |
| Alacritty | 950 | 79.0 | 46.4 |
| foot | 962 | 79.0 | 46.2 |
| Ghostty | 861 | 60.0 | 36.6 |
| Ptyxis | 303 | 59.0 | 40.5 |
| konsole | 418 | 21.0 | 13.9 |
| xfce4-terminal | 233 | 20.0 | 17.4 |
| st | 140 | 13.5 | 13.5 |

The two columns measure different things. Written-to-PTY is DOOM-fire's own
reported FPS: how fast it can push frames into the terminal. Presented FPS is
how often the terminal actually repainted. Reliabilitty, Alacritty and foot all
reach about 79 fps, which is the panel refresh rate, so they repaint on every
display frame. Ghostty and Ptyxis stop at 60 fps. Reliabilitty writes the most
frames because its renderer coalesces bursts into one paint per display frame;
the frame interval is derived from the display's refresh rate through RandR, so
rtty paints at most once per frame instead of wasting work on frames the
compositor would never show.

foot runs under 12to11, so its compositor's rendering work is not counted. The
draw, CPU and vtebench results above remain the better evidence for the speed
claims. DOOM-fire's `mem:` line is the per-frame output buffer size (about 16
KiB minimum, 96 KiB average and 100 KiB maximum in these runs), not process RAM.

## Ghostty and GTK4

The tables above use a clean config so every terminal starts on the same
footing. With the real config, Ghostty alone takes about 1.7 s to launch
versus ~160 ms clean; the others do not use GTK4 (st: Xlib, Alacritty:
winit/glutin, Reliabilitty: x11rb, xfce4-terminal: GTK3, foot: fcft/Wayland,
Konsole: Qt) and are unaffected.

The cause is a GTK4 bug, not Ghostty: `GApplication::startup` does a themed
window-icon lookup that loads the whole icon theme, and its cache parser is
quadratic. The active theme, `Papirus-Dark`, has roughly 102 sub-directories
and 297,000 cache entries, so startup does about 30 million inspections and
loads the theme twice. See
[GTK4-slow-launch-speed-test](https://github.com/Samueru-sama/GTK4-slow-launch-speed-test)
for the investigation and a minimal reproduction.
