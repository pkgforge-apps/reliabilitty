# Why Reliabilitty?

## Why not Alacritty?

Alacritty is fast, polished and has a good config file. I used it for years.

The problem is that it depends on a working GPU/OpenGL stack even when all I
need is a shell. I hit this [bug](https://github.com/alacritty/alacritty/issues/8566)

People have asked for a [software renderer](https://github.com/alacritty/alacritty/issues/8600).
The maintainer's answer was:

> If hardware accelerated rendering isn't available, then maybe Alacritty just
> isn't the right choice as a terminal emulator for that purpose.

## Why not st?

st is excellent as a base: small, simple and predictable. I used it after Alacritty.

Its patch system is painful in practice. There is no real configuration file,
the defaults live in `config.h` and must be recompiled with patches for simple features
like transparency and scrollback.

I love st's speed, reliability and simple design, so this project takes inspiration from it.

## Why not Ghostty?

Ghostty has the same basic problem as Alacritty: no software renderer. If the
GPU path is unavailable or broken, the terminal is not usable.

I used the GLFW builds before they were dropped. They were much faster and more
responsive than the GTK builds on my machines. In
[this discussion](https://github.com/ghostty-org/ghostty/discussions/5592#discussioncomment-13186436)
I posted a launch-time comparison: GLFW launched in about 100 ms, while the GTK
build took about 1 second. The GLFW backend was removed anyway in favor of
GTK4. Gross.

## What Reliabilitty is instead

Reliabilitty is meant to be reliable (heh), simple, small and as fast as possible:

- X11 only, using x11rb directly; no libX11 or XCB.
- CPU rendering with swash; no OpenGL, Vulkan, EGL or GPU process.
- MIT-SHM frame upload with a `PutImage` fallback.
- A shell-like config with live reload and env overrides.
- A static musl binary around 2.2 MiB.
- A full VT parser, scrollback, selection, clipboard, mouse reporting,
  truecolor, emoji fallback and background transparency.

It is meant to be a terminal emulator that you can count on to always launch!

## Feature comparison

| Feature | Reliabilitty | st | Alacritty | Ghostty |
| --- | --- | --- | --- | --- |
| Config file | rtty.conf | needs patch | TOML | key=value |
| Live reload | yes | no | yes | yes |
| Env overrides | yes | no | no | no |
| Scrollback | yes | needs patch | yes | yes |
| Scrollbar | yes | needs patch | no | yes |
| Truecolor | yes | needs patch | yes | yes |
| Transparency | yes, background | needs patch | yes, whole window incl. text | yes, background |
| Mouse reporting | yes | yes | yes | yes |
| OSC 52 clipboard | yes | needs patch | yes | yes |
| Search / Vi mode | yes / no | no | yes | yes |
| Ligatures | no | no | no | yes |

See [PERFORMANCE.md](PERFORMANCE.md) for benchmarks.

