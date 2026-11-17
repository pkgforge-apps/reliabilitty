# Performance

A simple benchmark of Reliabilitty against st, Alacritty and Ghostty. The
numbers are indicative, not a full benchmark suite.

## Environment

- Artix Linux, X11 with i3 and a compositor.
- Real desktop session, one terminal running at a time.
- Clean `$XDG_CONFIG_HOME` under `/tmp` for every terminal. Ghostty behaves
  very differently with the real config; see "Ghostty and GTK4" below.
- Same machine for every run.

## Versions

| Terminal | Version | Renderer |
|---|---|---|
| Reliabilitty | 0.1.0 | CPU, swash |
| st | 0.9.3 | CPU, Xft |
| Alacritty | 0.17.0 | GPU, OpenGL |
| Ghostty | 1.4.0-arch1 | GPU, OpenGL |

## Method

- Launch time: process spawn until the active window class matches, the same
  method as the `bench-startup` helper. Median of 3 runs.
- RSS and PSS: sum of the process tree after one second idle. Median of 3
  runs.
- Draw time: spawn until the final text frame appears after printing 10000
  lines and hiding the cursor. The right 20 pixels are excluded from the
  screenshot comparison to avoid scrollbar animations. Median of 3 runs.
- CPU: user plus system CPU time of the whole process tree, from spawn until
  the final frame. The percentage is that CPU time divided by wall time.
  Median of 3 runs.

## Results

### Launch time

| # | Terminal | Launch time |
|--:|---|---:|
| 1 | Reliabilitty | 31.3 ms |
| 2 | st | 38.9 ms |
| 3 | Alacritty | 120.6 ms |
| 4 | Ghostty | 180.4 ms |

### RSS

| # | Terminal | RSS |
|--:|---|---:|
| 1 | Reliabilitty | 14.1 MiB |
| 2 | st | 15.2 MiB |
| 3 | Alacritty | 94.3 MiB |
| 4 | Ghostty | 153.7 MiB |

### PSS

| # | Terminal | PSS |
|--:|---|---:|
| 1 | st | 4.6 MiB |
| 2 | Reliabilitty | 9.5 MiB |
| 3 | Alacritty | 42.5 MiB |
| 4 | Ghostty | 91.4 MiB |

### 10000 lines

| # | Terminal | Draw time |
|--:|---|---:|
| 1 | st | 181.1 ms |
| 2 | Reliabilitty | 315.2 ms |
| 3 | Alacritty | 423.6 ms |
| 4 | Ghostty | 461.4 ms |

### CPU while drawing 10000 lines

| # | Terminal | CPU time | CPU / wall |
|--:|---|---:|---:|
| 1 | st | 29.1 ms | 17% |
| 2 | Reliabilitty | 32.0 ms | 10% |
| 3 | Alacritty | 125.0 ms | 30% |
| 4 | Ghostty | 287.1 ms | 59% |

Reliabilitty uses the least CPU as a share of wall time. Its absolute CPU
time is close to st, which draws faster and so finishes sooner. The GPU
terminals burn several times more CPU on the same text.

## Combined

| Terminal | Launch | RSS | PSS | Draw | CPU |
|---|---:|---:|---:|---:|---:|
| Reliabilitty | 31.3 ms | 14.1 MiB | 9.5 MiB | 315.2 ms | 32.0 ms (10%) |
| st | 38.9 ms | 15.2 MiB | 4.6 MiB | 181.1 ms | 29.1 ms (17%) |
| Alacritty | 120.6 ms | 94.3 MiB | 42.5 MiB | 423.6 ms | 125.0 ms (30%) |
| Ghostty | 180.4 ms | 153.7 MiB | 91.4 MiB | 461.4 ms | 287.1 ms (59%) |

Reliabilitty is fastest to launch, and has the lowest RSS and CPU share. st
wins PSS, draw time and total CPU time because it is a smaller C process.
Both CPU terminals are far lighter than the GPU terminals in this workload.

The GPU terminals pay for their renderer, toolkit and GPU stack even before
text is drawn. That does not make GPU rendering useless in every case, but
for a normal terminal workload the CPU path is already cheap enough that the
extra machinery buys very little.

## Ghostty and GTK4

The table above uses a clean config so every terminal starts on the same
footing. With the real config, Ghostty alone takes about 1.7 s to launch
(measured repeatedly, matching the `bench-startup` result). The other
terminals are unaffected because none of them use GTK4:

- st uses Xlib directly.
- Alacritty uses winit/glutin.
- Reliabilitty uses x11rb.

The cause is a GTK4 bug, not Ghostty. GTK4's `GApplication::startup` does a
themed window-icon lookup before the window appears, which forces the whole
icon theme to load. The cache parser is quadratic: for each of the theme's
sub-directories it sweeps every entry in the icon cache. The active theme
here is `Papirus-Dark`, with roughly 102 sub-directories and 297,000 cache
entries, about 30 million inspections. GTK4 also loads the theme twice during
startup. The main thread just waits in `futex` for the worker thread doing
that CPU-bound parse, so there is no slow I/O to see in a syscall trace.

With a clean config GTK uses a small default icon theme, so Ghostty lands at
the ~180 ms GTK4 floor. The 1.7 s result comes from loading `Papirus-Dark`,
not from the shell or the number of config files.

See [GTK4-slow-launch-speed-test](https://github.com/Samueru-sama/GTK4-slow-launch-speed-test)
for the full investigation and a minimal GTK3/GTK4 reproduction.
