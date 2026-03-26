import json
import subprocess
from collections import Counter
import re

res = subprocess.run(['git', 'show', '0d5f8a0d16bf:benchmark_results.json'], capture_output=True, text=True, check=True)
data = json.loads(res.stdout)

missed_explanations = []
for item in data:
    if item['status'] == 'MISSED':
        missed_explanations.append(item['explanation'])

print(f"Total MISSED: {len(missed_explanations)}")
print("-" * 40)
for i, exp in enumerate(missed_explanations):
    print(f"[{i+1}] {exp}")
