"""Render the existing licence Markdown into Unicode RTF for NSIS RichEdit.

Uses only the standard library. The original notices remain in the package; this
is a display conversion, with actual table rows rather than Markdown pipes.
"""
import argparse
import re
from pathlib import Path


def escape(text):
    result = []
    data = text.encode("utf-16-le")
    for i in range(0, len(data), 2):
        unit = int.from_bytes(data[i:i + 2], "little")
        if unit in (92, 123, 125):
            result.append("\\" + chr(unit))
        elif 32 <= unit < 127:
            result.append(chr(unit))
        elif unit == 10:
            result.append("\\line ")
        else:
            result.append(f"\\u{unit if unit < 32768 else unit - 65536}?")
    return "".join(result)


def inline(text):
    if text.startswith("_") and text.endswith("_") and len(text) > 2:
        return "{\\i " + inline(text[1:-1]) + "}"
    # Keep the full destination visible so licence/source links survive conversion.
    text = re.sub(r"\[([^\]]+)\]\(([^)]+)\)", r"\1 (\2)", text)
    tokens = re.split(r"(\*\*.*?\*\*|`[^`]+`|(?<!\*)\*[^*\n]+\*(?!\*))", text)
    output = []
    for token in tokens:
        if token.startswith("**") and token.endswith("**"):
            output.append("{\\b " + escape(token[2:-2]) + "}")
        elif token.startswith("`") and token.endswith("`"):
            output.append("{\\f1 " + escape(token[1:-1]) + "}")
        elif token.startswith("*") and token.endswith("*"):
            output.append("{\\i " + escape(token[1:-1]) + "}")
        else:
            output.append(escape(token))
    return "".join(output)


def table_row(cells, header=False):
    # The licence control is narrow; use compact cells that can wrap independently.
    widths = {2: [1800, 6000], 3: [1650, 2850, 6000]}.get(len(cells))
    if widths is None:
        widths = [(i + 1) * 6000 // len(cells) for i in range(len(cells))]
    borders = "\\clbrdrb\\brdrs\\brdrw5\\brdrcf2"
    row = "\\trowd\\trgaph90\\trleft0 "
    row += "".join(borders + ("\\clcbpat3" if header else "") + f"\\cellx{end}" for end in widths)
    row += "\r\n"
    for cell in cells:
        row += "\\pard\\intbl\\f0\\fs16\\b0 " + ("\\b " if header else "") + inline(cell) + "\\cell "
    return row + "\\row\r\n\\pard\\b0\\f0\\fs18 "


def render_markdown(text):
    lines = text.splitlines()
    output = []
    index = 0
    while index < len(lines):
        line = lines[index].strip()
        if not line:
            index += 1
            continue
        if line.startswith("```"):
            index += 1
            while index < len(lines) and not lines[index].strip().startswith("```"):
                output.append("\\pard\\f1\\fs16 " + escape(lines[index]) + "\\par\r\n")
                index += 1
            index += 1
            output.append("\\pard\\f0\\fs18 ")
            continue
        if line.startswith("|") and index + 1 < len(lines) and re.match(r"^\s*\|[\s:|\-]+\|\s*$", lines[index + 1]):
            output.append(table_row([c.strip() for c in line.strip("|").split("|")], header=True))
            index += 2
            while index < len(lines) and lines[index].lstrip().startswith("|"):
                output.append(table_row([c.strip() for c in lines[index].strip().strip("|").split("|")]))
                index += 1
            continue
        heading = re.match(r"^(#{1,6})\s+(.+)$", line)
        if heading:
            size = 28 if len(heading[1]) == 1 else 22
            output.append(f"\\pard\\sb160\\sa80\\b\\fs{size} " + inline(heading[2]) + "\\b0\\fs18\\par\r\n")
            index += 1
            continue
        if re.fullmatch(r"[-*_]{3,}", line):
            output.append("\\pard\\sa100\\par\r\n")
            index += 1
            continue
        paragraph = [line]
        index += 1
        while index < len(lines) and lines[index].strip() and not lines[index].lstrip().startswith(("#", "|", "- ", "```")):
            paragraph.append(lines[index].strip())
            index += 1
        output.append("\\pard\\sa100 " + inline(" ".join(paragraph)) + "\\par\r\n")
    return "".join(output)


def render(license_text, notices):
    header = (r"{\rtf1\ansi\ansicpg1252\deff0\uc1"
              r"{\fonttbl{\f0 Segoe UI;}{\f1 Consolas;}}"
              r"{\colortbl;\red35\green40\blue48;\red190\green195\blue203;\red239\green242\blue246;}"
              r"\viewkind4\paperw7920\margl120\margr120\f0\fs18 ")
    full_license = "\\pard\\sb180\\b\\fs22 Apache License 2.0\\b0\\fs16\\par\r\n"
    full_license += "".join("\\pard " + escape(line) + "\\par\r\n" for line in license_text.splitlines())
    return header + "\r\n" + render_markdown(notices) + full_license + "}\r\n"


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    args.output.write_bytes(render((root / "LICENSE").read_text(encoding="utf-8"),
                                  (root / "docs/licenses.md").read_text(encoding="utf-8")).encode("ascii"))
