#!/usr/bin/env python3
"""Generate crates/yshell-app/assets/open_source_licenses.txt with full texts.

Collects:
  1) YShell GPL-3.0-only (root LICENSE)
  2) Bundled font license files under yshell-terminal/assets
  3) Each third-party crate's on-disk LICENSE* files from the cargo package dir

Usage (repo root):
  python scripts/licenses/generate_open_source_licenses.py
  python scripts/licenses/generate_open_source_licenses.py --target x86_64-pc-windows-gnu
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

LICENSE_NAME_PREFIXES = (
    "license",
    "copying",
    "copyright",
    "notice",
    "licence",
)

SKIP_SUFFIXES = (".md", ".html", ".htm", ".rtf")


def repo_root() -> Path:
    return Path(__file__).resolve().parents[2]


def run_cargo_metadata(root: Path, target: str | None) -> dict:
    cmd = [
        "cargo",
        "metadata",
        "--format-version",
        "1",
        "--manifest-path",
        str(root / "Cargo.toml"),
    ]
    if target:
        cmd.extend(["--filter-platform", target])
    proc = subprocess.run(cmd, cwd=root, capture_output=True, text=True, check=False)
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        raise SystemExit(f"cargo metadata failed ({proc.returncode})")
    return json.loads(proc.stdout)


def is_license_file(path: Path) -> bool:
    name = path.name.lower()
    if path.suffix.lower() in SKIP_SUFFIXES and name not in {
        "license.md",
        "licence.md",
        "copying.md",
        "notice.md",
    }:
        # Keep common markdown license names; skip other docs.
        if not name.startswith(LICENSE_NAME_PREFIXES):
            return False
    stem_ok = any(name == p or name.startswith(p) for p in LICENSE_NAME_PREFIXES)
    return path.is_file() and stem_ok


def collect_license_files(pkg_dir: Path) -> list[Path]:
    if not pkg_dir.is_dir():
        return []
    found: list[Path] = []
    for child in sorted(pkg_dir.iterdir(), key=lambda p: p.name.lower()):
        if is_license_file(child):
            found.append(child)
        # Some crates ship texts under LICENSES/ (case varies by registry FS).
        if child.is_dir() and child.name.lower() in {
            "license",
            "licenses",
            "licence",
            "licences",
        }:
            for nested in sorted(child.rglob("*")):
                if nested.is_file() and not nested.name.startswith("."):
                    found.append(nested)
    # De-dupe while preserving order.
    seen: set[Path] = set()
    out: list[Path] = []
    for p in found:
        rp = p.resolve()
        if rp not in seen:
            seen.add(rp)
            out.append(p)
    return out


def read_text(path: Path) -> str:
    data = path.read_bytes()
    for enc in ("utf-8", "utf-8-sig", "latin-1"):
        try:
            return data.decode(enc)
        except UnicodeDecodeError:
            continue
    return data.decode("utf-8", errors="replace")


def workspace_package_ids(meta: dict) -> set[str]:
    members = set(meta.get("workspace_members", []))
    return members


def third_party_packages(meta: dict) -> list[dict]:
    members = workspace_package_ids(meta)
    pkgs = []
    for pkg in meta["packages"]:
        if pkg["id"] in members:
            continue
        # Skip path/workspace-only tooling that is not a registry dep when present.
        pkgs.append(pkg)
    pkgs.sort(key=lambda p: (p["name"].lower(), p["version"]))
    return pkgs


def append_section(buf: list[str], title: str, body: str) -> None:
    buf.append("-" * 80)
    buf.append(title)
    buf.append("-" * 80)
    body = body.replace("\r\n", "\n").replace("\r", "\n").strip("\n")
    buf.append(body)
    buf.append("")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--target",
        default=None,
        help="cargo metadata --filter-platform triple (default: host)",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=None,
        help="output path (default: crates/yshell-app/assets/open_source_licenses.txt)",
    )
    args = parser.parse_args()
    root = repo_root()
    out_path = args.output or (
        root / "crates" / "yshell-app" / "assets" / "open_source_licenses.txt"
    )

    meta = run_cargo_metadata(root, args.target)
    target_label = args.target or "host"
    pkgs = third_party_packages(meta)

    lines: list[str] = []
    lines.append("YShell — Open source licenses")
    lines.append("=" * 80)
    lines.append("")
    lines.append(
        "This file is embedded into the About dialog at compile time "
        "(include_str!). It contains the full license texts for YShell, "
        "bundled fonts, and third-party crates used by this build."
    )
    lines.append("")
    lines.append(f"Generated for cargo metadata platform filter: {target_label}")
    lines.append(f"Third-party crates listed: {len(pkgs)}")
    lines.append("")

    # --- YShell -----------------------------------------------------------
    yshell_license = read_text(root / "LICENSE")
    append_section(
        lines,
        "YShell\nLicense: GPL-3.0-only",
        "YShell is licensed under the GNU General Public License, version 3 only "
        "(GPL-3.0-only).\n\n" + yshell_license,
    )

    # --- Bundled fonts ----------------------------------------------------
    font_dir = root / "crates" / "yshell-terminal" / "assets"
    for font_name, font_file in (
        ("Bundled font: DejaVu", "LICENSE-DejaVu.txt"),
        ("Bundled font: Noto Sans Mono CJK", "LICENSE-NotoSansMonoCJK-OFL-1.1.txt"),
    ):
        path = font_dir / font_file
        if path.is_file():
            append_section(lines, font_name, read_text(path))

    # --- Third-party crates -----------------------------------------------
    lines.append("=" * 80)
    lines.append("Third-party Rust crates")
    lines.append("=" * 80)
    lines.append("")
    lines.append(
        "Each crate section includes the SPDX license expression from the crate "
        "manifest and the full license file text(s) shipped with that package "
        "when present on disk. If a package omits license files, a standard "
        "SPDX text harvested from another crate in this dependency graph is "
        "substituted when available."
    )
    lines.append("")

    # Harvest canonical SPDX bodies from packages that do ship license files,
    # so packages that omit LICENSE* can still show a complete text.
    spdx_bodies: dict[str, str] = {}

    def remember_spdx_body(key: str, body: str) -> None:
        body = body.strip()
        if key and body and key not in spdx_bodies:
            spdx_bodies[key] = body

    for pkg in pkgs:
        pkg_dir = Path(pkg["manifest_path"]).parent
        for lf in collect_license_files(pkg_dir):
            name_l = lf.name.lower()
            text = read_text(lf)
            if "mit" in name_l:
                remember_spdx_body("MIT", text)
            elif "apache" in name_l:
                remember_spdx_body("Apache-2.0", text)
            elif "bsd-2" in name_l or name_l in {"license-bsd-2-clause", "bsd-2-clause"}:
                remember_spdx_body("BSD-2-Clause", text)
            elif "bsd-3" in name_l or "bsd" in name_l:
                remember_spdx_body("BSD-3-Clause", text)
            elif "bsl" in name_l or "boost" in name_l:
                remember_spdx_body("BSL-1.0", text)
            elif name_l in {"license", "licence", "copying"}:
                expr = (pkg.get("license") or "").upper()
                if expr == "MIT" or expr.startswith("MIT "):
                    remember_spdx_body("MIT", text)
                elif "APACHE-2.0" in expr and "MIT" not in expr:
                    remember_spdx_body("Apache-2.0", text)
                elif expr == "BSL-1.0":
                    remember_spdx_body("BSL-1.0", text)

    def fallback_texts_for_expression(expr: str) -> list[tuple[str, str]]:
        """Return (spdx_id, body) pairs for a crate license expression."""
        normalized = (
            expr.replace("/", " OR ")
            .replace(" OR ", "\n")
            .replace(" AND ", "\n")
            .replace("(", "")
            .replace(")", "")
        )
        tokens = [part.strip() for part in normalized.splitlines() if part.strip()]
        out: list[tuple[str, str]] = []
        seen: set[str] = set()
        for token in tokens:
            if token in seen:
                continue
            body = spdx_bodies.get(token)
            if body:
                seen.add(token)
                out.append((token, body))
        return out

    missing_files: list[str] = []
    for pkg in pkgs:
        name = pkg["name"]
        version = pkg["version"]
        expr = pkg.get("license") or "(no license field)"
        title = f"{name} {version}\nLicense: {expr}"
        pkg_dir = Path(pkg["manifest_path"]).parent
        license_files = collect_license_files(pkg_dir)
        if not license_files:
            fallbacks = fallback_texts_for_expression(expr)
            if fallbacks:
                parts = [
                    "(Package did not ship a LICENSE/COPYING/NOTICE file; "
                    "showing standard SPDX text(s) for the declared expression.)",
                    "",
                ]
                for spdx_id, body in fallbacks:
                    parts.append(f"----- SPDX: {spdx_id} -----")
                    parts.append(body.rstrip("\n"))
                    parts.append("")
                append_section(lines, title, "\n".join(parts).rstrip() + "\n")
            else:
                missing_files.append(f"{name} {version} ({expr})")
                body = (
                    f"(No LICENSE/COPYING/NOTICE file found in package directory:\n"
                    f" {pkg_dir})\n\n"
                    f"Declared license expression: {expr}"
                )
                append_section(lines, title, body)
            continue

        parts = []
        for lf in license_files:
            try:
                rel = lf.relative_to(pkg_dir).as_posix()
            except ValueError:
                rel = lf.name
            parts.append(f"----- file: {rel} -----")
            parts.append(read_text(lf).rstrip("\n"))
            parts.append("")
        append_section(lines, title, "\n".join(parts).rstrip() + "\n")

    out_path.parent.mkdir(parents=True, exist_ok=True)
    text = "\n".join(lines).rstrip() + "\n"
    out_path.write_text(text, encoding="utf-8", newline="\n")

    print(f"Wrote {out_path} ({len(text):,} bytes, {len(pkgs)} crates)")
    if missing_files:
        print(f"Warning: {len(missing_files)} crates had no on-disk license file:")
        for item in missing_files[:20]:
            print(f"  - {item}")
        if len(missing_files) > 20:
            print(f"  ... and {len(missing_files) - 20} more")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
