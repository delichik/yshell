# Application Icons (`gTerm`)

This directory holds the application and multi-binary role icons for **gTerm**,
following the WinUI 3 / Fluent 2 visual baseline:

- **`gterm.ico` / `gterm.icns` / `svg/gterm.svg` / `linux/*/gterm.png`**: Main
  application icon (`gterm` Host entry, installers, desktop shortcuts, taskbar,
  and window titlebar).
- **`gterm-ui.ico` / `svg/gterm-ui.svg`**: GUI process binary (`gterm-ui`) —
  neutral Fluent desktop window frame + bottom-right `gterm` badge. At runtime,
  `MainWindow` and `AboutWindow` still set their window/taskbar icon to the
  full main `gterm` icon.
- **`gterm-session-worker.ico` / `svg/gterm-session-worker.svg`**: Session
  worker binary (`gterm-session-worker`) — neutral terminal frame + `gterm`
  badge.
- **`gterm-mcp-daemon.ico` / `svg/gterm-mcp-daemon.svg`**: MCP daemon binary
  (`gterm-mcp-daemon`) — neutral service/protocol hub + `gterm` badge.

## Regenerating

```bash
python scripts/app-icons/generate.py
```

The generator uses only the Python 3 standard library and updates:
- `assets/icons/svg/*.svg`
- `assets/icons/*.ico` (16, 24, 32, 48, 64, 256)
- `assets/icons/gterm.icns`
- `assets/icons/linux/{16x16,32x32,48x48,128x128,256x256,512x512}/gterm.png`
- `ui/icons/brand/gterm-{32,64,96,256}.png`
