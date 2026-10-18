#!/usr/bin/env python3
"""Sync and validate YShell's gettext catalogs.

The template ``translations/yshell-app.pot`` is generated from
``ui/**/*.slint`` by ``scripts/i18n/extract.sh`` (slint-tr-extractor).  This
script merges that template into the two checked-in catalogs and validates
that the catalogs and the template agree.

Workflow (run from the repository root, WSL on Windows):

    bash scripts/i18n/extract.sh                 # 1. regenerate the .pot
    python3 scripts/i18n/catalogs.py sync        # 2. merge into both .po files
    # translate any newly added (empty) zh-CN msgstr entries
    bash scripts/i18n/check.sh                   # 3. validate everything

``sync`` keeps existing translations, drops entries that no longer exist in
the template, and adds new entries in template order.  en-US is a baseline
(msgstr == msgid); new zh-CN entries are left empty so that the check fails
until they are translated.

``check`` compares a regenerated template (``--pot``) against the committed
template and both catalogs and fails on missing entries, obsolete entries,
empty translations, broken headers, duplicate keys, or missing msgctxt.
It never touches the filesystem and requires no network access.

Only the Python standard library is used.
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
POT = ROOT / "translations" / "yshell-app.pot"
CATALOGS = {
    "en-US": ROOT / "translations" / "en-US" / "LC_MESSAGES" / "yshell-app.po",
    "zh-CN": ROOT / "translations" / "zh-CN" / "LC_MESSAGES" / "yshell-app.po",
}
NPLURALS = {"en-US": 2, "zh-CN": 1}
PLURAL_FORMS = {"en-US": "(n != 1)", "zh-CN": "0"}

# Order used when writing the .po header; unknown fields are appended.
HEADER_ORDER = [
    "Project-Id-Version",
    "POT-Creation-Date",
    "PO-Revision-Date",
    "Last-Translator",
    "Language-Team",
    "Language",
    "MIME-Version",
    "Content-Type",
    "Content-Transfer-Encoding",
    "Plural-Forms",
]
REQUIRED_HEADER_FIELDS = [
    "Project-Id-Version",
    "POT-Creation-Date",
    "PO-Revision-Date",
    "Last-Translator",
    "Language-Team",
    "Language",
    "MIME-Version",
    "Content-Type",
    "Content-Transfer-Encoding",
    "Plural-Forms",
]

_FIELD_RE = re.compile(r'^(msgctxt|msgid_plural|msgid|msgstr(?:\[(\d+)\])?)[ \t]+"(.*)"[ \t]*$')
_CONT_RE = re.compile(r'^"(.*)"[ \t]*$')
_CATALOG_LINE_RE = re.compile(
    r'^(?:#|msgctxt[ \t]|msgid[ \t]|msgid_plural[ \t]|msgstr(?:\[\d+\])?[ \t]|")'
)
_ESCAPES = {
    "n": "\n",
    "t": "\t",
    "r": "\r",
    '"': '"',
    "\\": "\\",
    "a": "\a",
    "b": "\b",
    "f": "\f",
    "v": "\v",
}


class CatalogError(Exception):
    """A catalog is malformed or inconsistent."""


def unescape(text: str) -> str:
    """Decode the C-style escapes used in gettext quoted strings."""
    out: list[str] = []
    i = 0
    while i < len(text):
        char = text[i]
        if char != "\\":
            out.append(char)
            i += 1
            continue
        i += 1
        if i >= len(text):
            out.append("\\")
            break
        escape = text[i]
        i += 1
        if escape in _ESCAPES:
            out.append(_ESCAPES[escape])
        elif escape == "x":
            match = re.match(r"[0-9A-Fa-f]{1,2}", text[i:])
            if not match:
                raise CatalogError("invalid \\x escape in catalog string")
            out.append(chr(int(match.group(0), 16)))
            i += len(match.group(0))
        elif escape in "01234567":
            digits = escape
            while len(digits) < 3 and i < len(text) and text[i] in "01234567":
                digits += text[i]
                i += 1
            out.append(chr(int(digits, 8)))
        else:
            # Unknown escapes are kept literally rather than rejected: gettext
            # never emits them, but a stray backslash should not hide the rest.
            out.append(escape)
    return "".join(out)


def escape(text: str) -> str:
    """Encode a string for a gettext quoted string."""
    return (
        text.replace("\\", "\\\\")
        .replace('"', '\\"')
        .replace("\n", "\\n")
        .replace("\t", "\\t")
        .replace("\r", "\\r")
    )


@dataclass
class Entry:
    msgid: str
    msgctxt: str | None = None
    msgid_plural: str | None = None
    msgstr: dict[int, str] = field(default_factory=dict)
    comments: list[str] = field(default_factory=list)

    def key(self) -> tuple[str | None, str, str | None]:
        return (self.msgctxt, self.msgid, self.msgid_plural)

    def describe(self) -> str:
        context = f' msgctxt "{self.msgctxt}"' if self.msgctxt else ""
        return f'{context} msgid "{self.msgid}"'.strip()


@dataclass
class Catalog:
    header_comments: list[str]
    header_fields: dict[str, str]
    entries: list[Entry]
    obsolete_blocks: int = 0


def _split_comment(comments: list[str]) -> tuple[list[str], list[str], list[str], list[str]]:
    """Return (translator, extracted, references, flags) comments."""
    translator: list[str] = []
    extracted: list[str] = []
    references: list[str] = []
    flags: list[str] = []
    for comment in comments:
        if comment.startswith("#:"):
            references.append(comment)
        elif comment.startswith("#."):
            extracted.append(comment)
        elif comment.startswith("#,"):
            flags.append(comment)
        elif comment.startswith("#|"):
            # Previous msgid comments are a msgmerge artifact; they carry no
            # meaning for the catalog and are dropped on rewrite.
            continue
        elif comment.startswith("#") and not comment.startswith("#~"):
            translator.append(comment)
    return translator, extracted, references, flags


def parse_catalog(path: Path) -> Catalog:
    """Parse a .po/.pot file into a :class:`Catalog`."""
    try:
        text = path.read_text(encoding="utf-8-sig")
    except OSError as exc:
        raise CatalogError(f"cannot read {path}: {exc}") from exc

    lines = text.splitlines()
    header_comments: list[str] = []
    header_fields: dict[str, str] = {}
    entries: list[Entry] = []
    obsolete_blocks = 0

    pending_comments: list[str] = []
    current: Entry | None = None
    last_field: tuple[str, int] | None = None
    in_obsolete = False

    def finish() -> None:
        nonlocal current, pending_comments
        if current is None:
            return
        if current.msgid == "" and current.msgctxt is None and current.msgid_plural is None:
            # Header entry.
            header_comments.extend(pending_comments)
            pending_comments = []
            header = current.msgstr.get(0, "")
            for line in header.split("\n"):
                if not line:
                    continue
                if ":" not in line:
                    raise CatalogError(
                        f"{path}: malformed header line {line!r} (missing ':')"
                    )
                key, value = line.split(":", 1)
                header_fields[key.strip()] = value.strip()
        else:
            current.comments = pending_comments
            pending_comments = []
            entries.append(current)
        current = None

    for lineno, line in enumerate(lines, 1):
        if not line.strip():
            if current is not None:
                finish()
            last_field = None
            in_obsolete = False
            continue
        if line.startswith("#~"):
            # Obsolete entry (msgmerge marks dropped messages with "#~").
            if not in_obsolete:
                obsolete_blocks += 1
                in_obsolete = True
            last_field = None
            continue
        if line.startswith("#"):
            if current is not None and current.msgid != "":
                finish()
            pending_comments.append(line)
            last_field = None
            continue
        match = _FIELD_RE.match(line)
        if match:
            name = match.group(1)
            index = int(match.group(2)) if match.group(2) is not None else 0
            value = unescape(match.group(3))
            if name == "msgctxt":
                if current is not None and current.msgid != "":
                    finish()
                if current is None:
                    current = Entry(msgid="")
                current.msgctxt = value
                last_field = ("msgctxt", 0)
            elif name == "msgid":
                if current is not None and current.msgid != "":
                    finish()
                if current is None:
                    current = Entry(msgid="")
                current.msgid = value
                last_field = ("msgid", 0)
            elif name == "msgid_plural":
                if current is None:
                    raise CatalogError(f"{path}:{lineno}: msgid_plural without msgid")
                current.msgid_plural = value
                last_field = ("msgid_plural", 0)
            else:  # msgstr / msgstr[n]
                if current is None:
                    raise CatalogError(f"{path}:{lineno}: msgstr without msgid")
                current.msgstr[index] = value
                last_field = ("msgstr", index)
            continue
        cont = _CONT_RE.match(line)
        if cont:
            if current is None or last_field is None:
                raise CatalogError(f"{path}:{lineno}: string continuation without field")
            name, index = last_field
            value = unescape(cont.group(1))
            if name == "msgctxt":
                current.msgctxt = (current.msgctxt or "") + value
            elif name == "msgid":
                current.msgid += value
            elif name == "msgid_plural":
                current.msgid_plural = (current.msgid_plural or "") + value
            else:
                current.msgstr[index] = current.msgstr.get(index, "") + value
            continue
        raise CatalogError(f"{path}:{lineno}: unrecognized catalog line {line!r}")

    if current is not None:
        finish()
    if pending_comments:
        # Trailing comments with no entry: keep them out of the catalog.
        pass

    return Catalog(header_comments, header_fields, entries, obsolete_blocks)


def _location_refs(comments: list[str]) -> list[str]:
    refs: list[str] = []
    for comment in comments:
        if not comment.startswith("#:"):
            continue
        for ref in comment[2:].split():
            if ref not in refs:
                refs.append(ref)
    return refs


def render_catalog(catalog: Catalog) -> str:
    """Serialize a :class:`Catalog` back to gettext text (LF endings)."""
    out: list[str] = []
    out.extend(catalog.header_comments)
    out.append('msgid ""')
    out.append('msgstr ""')
    fields = dict(catalog.header_fields)
    for key in HEADER_ORDER:
        if key in fields:
            out.append(f'"{key}: {escape(fields[key])}\\n"')
    for key, value in fields.items():
        if key not in HEADER_ORDER:
            out.append(f'"{key}: {escape(value)}\\n"')
    out.append("")

    for entry in catalog.entries:
        translator, extracted, refs, flags = _split_comment(entry.comments)
        out.extend(translator)
        out.extend(extracted)
        refs = _location_refs(refs)
        if refs:
            out.append("#: " + " ".join(refs))
        out.extend(flags)
        if entry.msgctxt is not None:
            out.append(f'msgctxt "{escape(entry.msgctxt)}"')
        out.append(f'msgid "{escape(entry.msgid)}"')
        if entry.msgid_plural is not None:
            out.append(f'msgid_plural "{escape(entry.msgid_plural)}"')
            for index in sorted(entry.msgstr):
                out.append(f'msgstr[{index}] "{escape(entry.msgstr[index])}"')
        else:
            out.append(f'msgstr "{escape(entry.msgstr.get(0, ""))}"')
        out.append("")

    return "\n".join(out)


def _keyed(entries: list[Entry], label: str) -> dict[tuple[str | None, str, str | None], Entry]:
    keyed: dict[tuple[str | None, str, str | None], Entry] = {}
    for entry in entries:
        if entry.key() in keyed:
            raise CatalogError(f"{label}: duplicate key {entry.describe()}")
        keyed[entry.key()] = entry
    return keyed


def _load_pot(path: Path) -> tuple[Catalog, dict[tuple[str | None, str, str | None], Entry]]:
    catalog = parse_catalog(path)
    if catalog.obsolete_blocks:
        raise CatalogError(f"{path}: template contains {catalog.obsolete_blocks} obsolete entry block(s)")
    if not catalog.entries:
        raise CatalogError(f"{path}: template contains no messages")
    keyed = _keyed(catalog.entries, str(path))
    return catalog, keyed


def normalize_pot_text(text: str) -> tuple[str, int]:
    """Make slint-tr-extractor output valid gettext.

    slint-tr-extractor 1.18 wraps long ``//`` source comments in ``#.``
    comments but forgets the ``#. `` prefix on the wrapped lines (and sometimes
    drops it on the first line too).  gettext treats the bare text as invalid
    syntax.  Prefix such lines so they stay extracted comments.
    """
    lines = text.splitlines()
    fixed: list[str] = []
    changed = 0
    for line in lines:
        if line.strip() and not _CATALOG_LINE_RE.match(line):
            fixed.append("#. " + line)
            changed += 1
        else:
            fixed.append(line)
    return "\n".join(fixed) + "\n", changed


def command_normalize(args: argparse.Namespace) -> int:
    for name in args.files:
        path = Path(name)
        try:
            text = path.read_text(encoding="utf-8")
        except OSError as exc:
            raise CatalogError(f"cannot read {path}: {exc}") from exc
        normalized, changed = normalize_pot_text(text)
        if changed:
            path.write_text(normalized, encoding="utf-8", newline="\n")
        print(f"{path}: normalized {changed} comment line(s)")
    return 0


def command_sync(args: argparse.Namespace) -> int:
    pot_catalog, pot_entries = _load_pot(POT)
    pot_date = pot_catalog.header_fields.get("POT-Creation-Date", "")
    languages = [args.lang] if args.lang else list(CATALOGS)

    for lang in languages:
        path = CATALOGS[lang]
        old_catalog = parse_catalog(path)
        old_entries = _keyed(old_catalog.entries, str(path))
        old_by_key = dict(old_entries)
        old_by_msgid: dict[str, list[Entry]] = {}
        for entry in old_catalog.entries:
            if any(entry.msgstr.values()):
                old_by_msgid.setdefault(entry.msgid, []).append(entry)
        nplurals = NPLURALS[lang]

        merged: list[Entry] = []
        added: list[Entry] = []
        kept = 0
        reset = 0
        reused = 0
        for pot_entry in pot_catalog.entries:
            old = old_by_key.pop(pot_entry.key(), None)
            translator, extracted, references, flags = (
                _split_comment(old.comments) if old is not None else ([], [], [], [])
            )
            locations = _location_refs(pot_entry.comments)
            comments = translator + extracted
            if locations:
                comments.append("#: " + " ".join(locations))
            comments.extend(flags)

            if lang == "en-US":
                if pot_entry.msgid_plural is not None:
                    msgstr = {0: pot_entry.msgid, 1: pot_entry.msgid_plural}
                else:
                    msgstr = {0: pot_entry.msgid}
            elif old is not None:
                msgstr = {index: old.msgstr.get(index, "") for index in range(nplurals)}
                reset += sum(1 for value in msgstr.values() if value == "")
            else:
                reuse = None
                if args.reuse_same_msgid:
                    # A renamed component changes msgctxt but keeps the text;
                    # carry the translation over when the whole msgid matches.
                    for candidate in old_by_msgid.get(pot_entry.msgid, []):
                        if candidate.msgid_plural == pot_entry.msgid_plural:
                            reuse = candidate
                            break
                if reuse is not None:
                    msgstr = {index: reuse.msgstr.get(index, "") for index in range(nplurals)}
                    reused += 1
                else:
                    msgstr = {index: "" for index in range(nplurals)}
                    reset += sum(1 for value in msgstr.values() if value == "")

            if old is None:
                added.append(pot_entry)
            else:
                kept += 1
            merged.append(
                Entry(
                    msgid=pot_entry.msgid,
                    msgctxt=pot_entry.msgctxt,
                    msgid_plural=pot_entry.msgid_plural,
                    msgstr=msgstr,
                    comments=comments,
                )
            )

        removed = list(old_by_key.values())
        header_fields = dict(old_catalog.header_fields)
        header_fields["POT-Creation-Date"] = pot_date
        header_fields.setdefault("Project-Id-Version", "yshell")
        header_fields.setdefault("PO-Revision-Date", pot_date)
        header_fields.setdefault("Last-Translator", "YShell")
        header_fields.setdefault("Language-Team", "YShell")
        header_fields["Language"] = lang
        header_fields["MIME-Version"] = "1.0"
        header_fields["Content-Type"] = "text/plain; charset=UTF-8"
        header_fields["Content-Transfer-Encoding"] = "8bit"
        header_fields["Plural-Forms"] = f"nplurals={nplurals}; plural={PLURAL_FORMS[lang]};"

        new_catalog = Catalog(
            header_comments=old_catalog.header_comments,
            header_fields=header_fields,
            entries=merged,
        )
        rendered = render_catalog(new_catalog)

        print(
            f"{lang}: {len(merged)} entries "
            f"(kept {kept}, added {len(added)}, removed {len(removed)}, "
            f"reused {reused}, empty {reset})"
        )
        for entry in removed:
            print(f"  - removed: {entry.describe()}")
        for entry in added:
            print(f"  + added: {entry.describe()}")
        if added:
            print(f"  ! {len(added)} new entries need translation")
        if args.dry_run:
            continue
        current_text = path.read_text(encoding="utf-8")
        if current_text == rendered:
            print(f"  unchanged {path.relative_to(ROOT)}")
        else:
            path.write_text(rendered, encoding="utf-8", newline="\n")
            print(f"  wrote {path.relative_to(ROOT)}")

    return 0


def _header_errors(lang: str, catalog: Catalog, path: Path) -> list[str]:
    errors: list[str] = []
    for name in REQUIRED_HEADER_FIELDS:
        if not catalog.header_fields.get(name):
            errors.append(f"{lang}: header field {name!r} is missing or empty in {path.name}")
    if catalog.header_fields.get("Language") != lang:
        errors.append(
            f"{lang}: header Language is {catalog.header_fields.get('Language')!r}, expected {lang!r}"
        )
    plural = catalog.header_fields.get("Plural-Forms", "")
    match = re.search(r"nplurals\s*=\s*(\d+)", plural)
    if not match or int(match.group(1)) != NPLURALS[lang]:
        errors.append(f"{lang}: header Plural-Forms {plural!r} does not declare nplurals={NPLURALS[lang]}")
    if "charset=UTF-8" not in catalog.header_fields.get("Content-Type", ""):
        errors.append(f"{lang}: header Content-Type must declare charset=UTF-8")
    return errors


def command_check(args: argparse.Namespace) -> int:
    errors: list[str] = []
    fresh_pot = Path(args.pot).resolve()
    committed_pot = Path(args.committed_pot).resolve()

    try:
        fresh_catalog, fresh_entries = _load_pot(fresh_pot)
        committed_catalog, committed_entries = _load_pot(committed_pot)
    except CatalogError as exc:
        print(f"FAIL: {exc}")
        return 1

    # The checked-in template must match the current ui/**/*.slint sources.
    missing_from_pot = sorted(set(fresh_entries) - set(committed_entries))
    extra_in_pot = sorted(set(committed_entries) - set(fresh_entries))
    if missing_from_pot:
        errors.append(
            f"template {committed_pot.name} is stale: {len(missing_from_pot)} source message(s) missing"
        )
        for key in missing_from_pot:
            errors.append(f"  stale .pot: {fresh_entries[key].describe()}")
    if extra_in_pot:
        errors.append(
            f"template {committed_pot.name} is stale: {len(extra_in_pot)} removed message(s) still present"
        )
        for key in extra_in_pot:
            errors.append(f"  stale .pot: {committed_entries[key].describe()}")

    # Every message must carry the context the extractor assigned to it.
    for entry in fresh_catalog.entries:
        if not entry.msgctxt:
            errors.append(f"template has a message without msgctxt: {entry.describe()}")

    counts: dict[str, tuple[int, int, int, int]] = {}
    for lang, path in CATALOGS.items():
        try:
            catalog = parse_catalog(path)
        except CatalogError as exc:
            errors.append(f"{lang}: {exc}")
            counts[lang] = (0, 0, 0, 0)
            continue

        errors.extend(_header_errors(lang, catalog, path))
        if catalog.obsolete_blocks:
            errors.append(
                f"{lang}: {catalog.obsolete_blocks} obsolete entry block(s) present; run sync to drop them"
            )
        try:
            keyed = _keyed(catalog.entries, str(path))
        except CatalogError as exc:
            errors.append(str(exc))
            counts[lang] = (len(catalog.entries), 0, 0, 0)
            continue

        missing = sorted(set(committed_entries) - set(keyed))
        obsolete = sorted(set(keyed) - set(committed_entries))
        empty = 0
        for key in sorted(set(committed_entries) & set(keyed)):
            entry = keyed[key]
            required_forms = NPLURALS[lang] if key[2] is not None else 1
            for index in range(required_forms):
                value = entry.msgstr.get(index, "")
                if not value:
                    empty += 1
                    errors.append(f"{lang}: empty msgstr[{index}]: {entry.describe()}")
            for index in sorted(entry.msgstr):
                if index >= required_forms:
                    errors.append(
                        f"{lang}: unexpected msgstr[{index}] (nplurals={required_forms}): {entry.describe()}"
                    )
            if lang == "en-US":
                if key[2] is None:
                    if entry.msgstr.get(0) != key[1]:
                        errors.append(f"en-US: msgstr must equal msgid: {entry.describe()}")
                else:
                    if entry.msgstr.get(0) != key[1] or entry.msgstr.get(1) != key[2]:
                        errors.append(f"en-US: msgstr must equal msgid/msgid_plural: {entry.describe()}")

        for key in missing:
            errors.append(f"{lang}: missing message: {committed_entries[key].describe()}")
        for key in obsolete:
            errors.append(f"{lang}: obsolete message: {keyed[key].describe()}")
        counts[lang] = (len(catalog.entries), len(missing), len(obsolete), empty)

    print(f"source entries (.pot): {len(committed_entries)}")
    if missing_from_pot or extra_in_pot:
        print(
            f"regenerated .pot: {len(fresh_entries)} entries "
            f"(stale: {len(missing_from_pot)} missing, {len(extra_in_pot)} extra)"
        )
    else:
        print(f"regenerated .pot: {len(fresh_entries)} entries (in sync with ui/**/*.slint)")
    for lang, (total, missing, obsolete, empty) in counts.items():
        print(
            f"{lang}: {total} entries "
            f"(missing {missing}, obsolete {obsolete}, empty {empty})"
        )

    if errors:
        print(f"\nFAIL: {len(errors)} issue(s)")
        for error in errors:
            print(f"  {error}")
        return 1
    print("\nOK: catalogs are in sync")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    subparsers = parser.add_subparsers(dest="command", required=True)

    sync = subparsers.add_parser("sync", help="merge the committed .pot into the .po files")
    sync.add_argument("--lang", choices=sorted(CATALOGS), help="only sync one language")
    sync.add_argument("--dry-run", action="store_true", help="report changes without writing files")
    sync.add_argument(
        "--reuse-same-msgid",
        action="store_true",
        help="copy a translation from a dropped entry with an identical msgid "
        "(useful after a component/context rename)",
    )
    sync.set_defaults(func=command_sync)

    normalize = subparsers.add_parser(
        "normalize", help="prefix stray extracted-comment lines in a generated .pot"
    )
    normalize.add_argument("files", nargs="+", help=".pot file(s) to normalize in place")
    normalize.set_defaults(func=command_normalize)

    check = subparsers.add_parser("check", help="validate the .pot and both .po files")
    check.add_argument(
        "--pot",
        default=str(POT),
        help="freshly regenerated .pot (default: the committed translations/yshell-app.pot)",
    )
    check.add_argument(
        "--committed-pot",
        default=str(POT),
        help="checked-in .pot to compare against the regenerated one",
    )
    check.set_defaults(func=command_check)

    args = parser.parse_args(argv)
    try:
        return args.func(args)
    except CatalogError as exc:
        print(f"FAIL: {exc}")
        return 1


if __name__ == "__main__":
    sys.exit(main())
