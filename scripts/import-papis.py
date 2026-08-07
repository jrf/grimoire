#!/usr/bin/env python3
"""One-shot script to import a papis library into grimoire."""

import os
import shutil
import sys
import yaml
from pathlib import Path

def slugify(text):
    import re
    text = text.lower()
    text = re.sub(r'[^a-z0-9\s-]', '', text)
    text = re.sub(r'[\s]+', '-', text.strip())
    return text

SKIP_WORDS = {"a", "an", "the", "on", "of", "for", "in", "to", "and", "with"}

def make_dir_name(authors, year, title):
    if authors:
        last = authors[0].split()[-1] if ' ' in authors[0] else authors[0]
    else:
        last = "unknown"
    y = str(year) if year else "0000"
    words = title.split() if title else ["untitled"]
    title_word = next((w for w in words if w.lower() not in SKIP_WORDS), words[0])
    return slugify(f"{last}-{y}-{title_word}")

def parse_authors(info):
    if "author_list" in info:
        authors = []
        for a in info["author_list"]:
            given = a.get("given", "")
            family = a.get("family", "")
            if given and family:
                authors.append(f"{given} {family}")
            elif family:
                authors.append(family)
        if authors:
            return authors
    if "author" in info:
        raw = info["author"]
        if " and " in raw:
            return [a.strip() for a in raw.split(" and ")]
        return [a.strip() for a in raw.split(",") if a.strip()]
    return []

def escape_toml_string(s):
    return s.replace('\\', '\\\\').replace('"', '\\"')

def write_info_toml(dest_dir, data):
    lines = []
    lines.append(f'title = "{escape_toml_string(data["title"])}"')
    if data["authors"]:
        authors_str = ", ".join(f'"{escape_toml_string(a)}"' for a in data["authors"])
        lines.append(f"authors = [{authors_str}]")
    if data["year"]:
        lines.append(f"year = {data['year']}")
    if data.get("doi"):
        lines.append(f'doi = "{escape_toml_string(data["doi"])}"')
    if data.get("arxiv"):
        lines.append(f'arxiv = "{escape_toml_string(data["arxiv"])}"')
    if data.get("journal"):
        lines.append(f'journal = "{escape_toml_string(data["journal"])}"')
    if data.get("tags"):
        tags_str = ", ".join(f'"{escape_toml_string(t)}"' for t in data["tags"])
        lines.append(f"tags = [{tags_str}]")
    if data.get("files"):
        files_str = ", ".join(f'"{escape_toml_string(f)}"' for f in data["files"])
        lines.append(f"files = [{files_str}]")
    if data.get("abstract"):
        lines.append(f'abstract = """\n{data["abstract"]}\n"""')

    (dest_dir / "info.toml").write_text("\n".join(lines) + "\n")

def import_papis(papis_dir, library_dir):
    papis_dir = Path(papis_dir).expanduser()
    library_dir = Path(library_dir).expanduser()
    library_dir.mkdir(parents=True, exist_ok=True)

    imported = 0
    skipped = 0

    for entry in sorted(papis_dir.iterdir()):
        if not entry.is_dir():
            continue
        info_path = entry / "info.yaml"
        if not info_path.exists():
            continue

        with open(info_path) as f:
            info = yaml.safe_load(f)

        if not info:
            skipped += 1
            continue

        title = info.get("title", "Untitled")
        authors = parse_authors(info)
        year = info.get("year")
        doi = info.get("doi")
        arxiv = info.get("arxiv") or info.get("eprint")
        journal = info.get("journal")
        tags = info.get("tags", [])
        abstract = info.get("abstract", "")

        dir_name = make_dir_name(authors, year, title)
        dest = library_dir / dir_name

        # Handle collisions
        if dest.exists():
            n = 2
            while True:
                candidate = library_dir / f"{dir_name}-{n}"
                if not candidate.exists():
                    dest = candidate
                    break
                n += 1

        dest.mkdir(parents=True)

        # Copy PDFs
        files = []
        pdf_list = info.get("files", [])
        for pdf_name in pdf_list:
            src = entry / pdf_name
            if src.exists():
                # Clean up double extensions like .pdf.pdf
                clean_name = pdf_name
                if clean_name.endswith(".pdf.pdf"):
                    clean_name = clean_name[:-4]
                shutil.copy2(src, dest / clean_name)
                files.append(clean_name)

        # Also check for any PDF not listed in files
        if not files:
            for f in entry.iterdir():
                if f.suffix.lower() == ".pdf":
                    shutil.copy2(f, dest / f.name)
                    files.append(f.name)

        data = {
            "title": title,
            "authors": authors,
            "year": year,
            "doi": doi,
            "arxiv": arxiv,
            "journal": journal,
            "tags": tags if isinstance(tags, list) else [],
            "files": files,
            "abstract": abstract if abstract else None,
        }

        write_info_toml(dest, data)
        imported += 1
        print(f"  {dir_name}")

    print(f"\nImported {imported} papers, skipped {skipped}")

if __name__ == "__main__":
    papis = sys.argv[1] if len(sys.argv) > 1 else "~/repos/papis_papers"
    library = sys.argv[2] if len(sys.argv) > 2 else "~/repos/papers"
    print(f"Importing from {papis} → {library}\n")
    import_papis(papis, library)
