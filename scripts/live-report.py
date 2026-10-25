"""Build an HTML progress report from live-test run folders.

Each run folder is what scripts/live-test.ps1 leaves behind: summary.json, PNG
screenshots, and optionally an MP4 from scripts/record-live.ps1. Runs are
given oldest first as LABEL=PATH; the report puts their gates side by side and
shows each run's screenshots and video.

    python scripts/live-report.py --out report "before (1532635)=runs/a" "now (8f9e466)=runs/b"
"""
import argparse
import html
import json
import shutil
from pathlib import Path

# Gates with a known good value. Everything else is shown as recorded; True is
# good and False is bad for the boolean scenario gates (h1_..., h2_..., ...).
EXPECTED = {
    'settle_positioning_per_s': lambda v, s: v == 0,
    'terminals_settle_positioning_per_s': lambda v, s: v == 0,
    'test_windows_moved_by_wiri': lambda v, s: v == 3,
    'ctl_timeouts': lambda v, s: v == 0,
    'unknown_key_warnings': lambda v, s: v == 0,
    'config_bind_warnings': lambda v, s: v == 0,
    'forms_overlapping_pairs': lambda v, s: v == 0,
    'activation_failure_lines': lambda v, s: v == 0,
    'ctl_failures_while_an_app_hung': lambda v, s: v == 0,
    'wiri_new_console_windows_at_start': lambda v, s: v == 0,
    'wiri_visible_windows_of_its_own': lambda v, s: v == 0,
    'windows_visible_after_quit': lambda v, s: v == s.get('windows_expected_after_quit'),
    'windows_visible_after_restart': lambda v, s: v == s.get('windows_hidden_while_wiri_dead', v),
    # The crash record is deleted once the windows are back, so False is the good value.
    'hidden_json_present_after_restart': lambda v, s: v is False,
}
INFO_ONLY = {'work_area', 'wiri_log_bytes', 'positioning_lines_total', 'ctl_max_ms',
             'ctl_max_ms_while_an_app_hung', 'max_positioning_per_s', 'terminal_windows_seen',
             'windows_expected_after_quit', 'wiri_subsystem', 'wiri_ctl_subsystem', 'forms_min_visible_width',
             'foreground_after_open', 'activation_ok_lines', 'h1_unknown_modifier_warnings',
             'h1_unsupported_modifier_warnings'}


def verdict(key, value, summary):
    if key in EXPECTED:
        try:
            return 'pass' if EXPECTED[key](value, summary) else 'fail'
        except Exception:
            return 'info'
    if key in INFO_ONLY:
        return 'info'
    if isinstance(value, bool):
        return 'pass' if value else 'fail'
    return 'info'


def load_run(label, path):
    path = Path(path)
    summary = {}
    sj = path / 'summary.json'
    if sj.exists():
        summary = json.loads(sj.read_text(encoding='utf-8-sig'))
    return {
        'label': label,
        'path': path,
        'summary': {k: v for k, v in summary.items() if k != 'steps'},
        'pngs': sorted(path.glob('*.png')),
        'videos': sorted(path.glob('*.mp4')),
    }


def cell(value, v):
    text = html.escape(str(value)) if value is not None else '&ndash;'
    return f'<td class="{v}">{text}</td>' if value is not None else '<td class="missing">&ndash;</td>'


def build(runs, out):
    out.mkdir(parents=True, exist_ok=True)
    assets = out / 'assets'
    # Rebuilt from scratch each time, so a run that moved position leaves no stale images.
    shutil.rmtree(assets, ignore_errors=True)
    keys = []
    for run in runs:
        for k in run['summary']:
            if k not in keys:
                keys.append(k)

    rows = []
    for k in keys:
        cells = []
        for run in runs:
            s = run['summary']
            if k in s:
                cells.append(cell(s[k], verdict(k, s[k], s)))
            else:
                cells.append(cell(None, 'missing'))
        rows.append(f'<tr><th scope="row">{html.escape(k)}</th>{"".join(cells)}</tr>')

    counts = []
    for run in runs:
        s = run['summary']
        verdicts = [verdict(k, v, s) for k, v in s.items()]
        counts.append((verdicts.count('pass'), verdicts.count('fail')))

    sections = []
    for i, run in enumerate(runs):
        dest = assets / f'run{i}'
        dest.mkdir(parents=True, exist_ok=True)
        figs = []
        for png in run['pngs']:
            shutil.copy2(png, dest / png.name)
            src = f'assets/run{i}/{png.name}'
            figs.append(f'<figure><a href="{src}"><img loading="lazy" src="{src}" alt="{html.escape(png.stem)}"></a>'
                        f'<figcaption>{html.escape(png.stem)}</figcaption></figure>')
        vids = []
        for mp4 in run['videos']:
            shutil.copy2(mp4, dest / mp4.name)
            vids.append(f'<video controls preload="metadata" src="assets/run{i}/{mp4.name}"></video>')
        passed, failed = counts[i]
        sections.append(
            f'<section><h2>{html.escape(run["label"])}</h2>'
            f'<p class="meta">{passed} gates pass, {failed} fail, {len(run["pngs"])} screenshots'
            f'{", " + str(len(run["videos"])) + " recording(s)" if run["videos"] else ""}</p>'
            f'{"".join(vids)}<div class="grid">{"".join(figs)}</div></section>')

    head = ''.join(f'<th scope="col">{html.escape(r["label"])}<br><span class="meta">'
                   f'{counts[i][0]} pass / {counts[i][1]} fail</span></th>' for i, r in enumerate(runs))
    page = f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>wiri live progress</title>
<style>
:root {{ --bg: #f6f5f2; --fg: #1d1d1b; --muted: #6b6b66; --line: #dcdad4; --card: #ffffff;
        --pass: #1f7a4d; --pass-bg: #e3f3ea; --fail: #a32020; --fail-bg: #fbe4e2; }}
@media (prefers-color-scheme: dark) {{
  :root {{ --bg: #141413; --fg: #ecebe6; --muted: #9a9993; --line: #33332f; --card: #1c1c1a;
          --pass: #6fd3a0; --pass-bg: #16301f; --fail: #ff8f86; --fail-bg: #3a1714; }} }}
* {{ box-sizing: border-box; }}
body {{ margin: 0; padding: 24px 16px 64px; background: var(--bg); color: var(--fg);
       font: 15px/1.5 "Segoe UI Variable", "Segoe UI", system-ui, sans-serif; }}
main {{ max-width: 1200px; margin: 0 auto; }}
h1 {{ font-size: 28px; margin: 0 0 4px; }} h2 {{ font-size: 20px; margin: 32px 0 4px; }}
.meta {{ color: var(--muted); font-size: 13px; font-weight: normal; }}
.table-wrap {{ overflow-x: auto; border: 1px solid var(--line); border-radius: 8px; background: var(--card); }}
table {{ border-collapse: collapse; width: 100%; font-size: 13px; }}
th, td {{ padding: 6px 10px; border-bottom: 1px solid var(--line); text-align: left; white-space: nowrap; }}
thead th {{ position: sticky; top: 0; background: var(--card); }}
tbody th {{ font-weight: normal; font-family: ui-monospace, Consolas, monospace; }}
td.pass {{ color: var(--pass); background: var(--pass-bg); }}
td.fail {{ color: var(--fail); background: var(--fail-bg); font-weight: 600; }}
td.missing {{ color: var(--muted); }}
.grid {{ display: grid; grid-template-columns: repeat(auto-fill, minmax(260px, 1fr)); gap: 12px; margin-top: 12px; }}
figure {{ margin: 0; background: var(--card); border: 1px solid var(--line); border-radius: 8px; overflow: hidden; }}
figure img {{ display: block; width: 100%; height: auto; }}
figcaption {{ padding: 6px 10px; font-size: 12px; color: var(--muted); font-family: ui-monospace, Consolas, monospace; }}
video {{ width: 100%; max-width: 960px; border-radius: 8px; border: 1px solid var(--line); margin-top: 12px; }}
</style></head><body><main>
<h1>wiri live progress</h1>
<p class="meta">Live desktop runs of scripts/live-test.ps1, oldest first. Green = expected value, red = regression or failure.</p>
<div class="table-wrap"><table><thead><tr><th scope="col">gate</th>{head}</tr></thead>
<tbody>{"".join(rows)}</tbody></table></div>
{"".join(sections)}
</main></body></html>
"""
    (out / 'index.html').write_text(page, encoding='utf-8')
    return out / 'index.html'


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--out', required=True, type=Path)
    ap.add_argument('runs', nargs='+', help='LABEL=PATH, oldest first')
    args = ap.parse_args()
    runs = []
    for spec in args.runs:
        label, _, path = spec.partition('=')
        runs.append(load_run(label, path))
    print(build(runs, args.out))


if __name__ == '__main__':
    main()
