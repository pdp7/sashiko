import json
import subprocess
import textwrap

def get_data(commit):
    res = subprocess.run(['git', 'show', f'{commit}:benchmark_results.json'], capture_output=True, text=True, check=True)
    return {item['commit']: {'status': item['status'], 'desc': item['problem_description']} for item in json.loads(res.stdout)}

baseline = get_data('9e668f4b7e83')
new = get_data('0d5f8a0d16bf')

all_commits = set(baseline.keys()) | set(new.keys())

transitions = {}
for c in all_commits:
    b_stat = baseline.get(c, {}).get('status', 'NOT_FOUND')
    n_stat = new.get(c, {}).get('status', 'NOT_FOUND')
    desc = new.get(c, {}).get('desc') or baseline.get(c, {}).get('desc', 'No description')
    transitions.setdefault((b_stat, n_stat), []).append((c, desc))

def print_group(desc_title, b_stat, n_stat):
    items = transitions.get((b_stat, n_stat), [])
    if items:
        print(f"### {desc_title} ({len(items)})")
        for c, desc in items:
            short_desc = textwrap.shorten(desc, width=100, placeholder="...")
            print(f"- {c[:12]}: {short_desc}")
        print()

print("## Improvements")
print_group("Missed -> Detected", "MISSED", "DETECTED")
print_group("Missed -> Partially Detected", "MISSED", "PARTIALLY_DETECTED")
print_group("Partially Detected -> Detected", "PARTIALLY_DETECTED", "DETECTED")

print("## Regressions")
print_group("Detected -> Missed", "DETECTED", "MISSED")
print_group("Detected -> Partially Detected", "DETECTED", "PARTIALLY_DETECTED")
print_group("Partially Detected -> Missed", "PARTIALLY_DETECTED", "MISSED")

print("## Unchanged Overlap")
print_group("Consistently Detected", "DETECTED", "DETECTED")
print_group("Consistently Partially Detected", "PARTIALLY_DETECTED", "PARTIALLY_DETECTED")
print_group("Consistently Missed", "MISSED", "MISSED")

print("## Other")
for (b, n), items in transitions.items():
    if (b, n) not in [("MISSED", "DETECTED"), ("MISSED", "PARTIALLY_DETECTED"), ("PARTIALLY_DETECTED", "DETECTED"),
                      ("DETECTED", "MISSED"), ("DETECTED", "PARTIALLY_DETECTED"), ("PARTIALLY_DETECTED", "MISSED"),
                      ("DETECTED", "DETECTED"), ("PARTIALLY_DETECTED", "PARTIALLY_DETECTED"), ("MISSED", "MISSED")]:
        if items:
            print(f"### {b} -> {n} ({len(items)})")
            for c, desc in items:
                short_desc = textwrap.shorten(desc, width=100, placeholder="...")
                print(f"- {c[:12]}: {short_desc}")
            print()
