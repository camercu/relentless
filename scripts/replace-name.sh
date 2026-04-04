#!/bin/zsh
# Replace all instances of 'tenacious' with 'relentless' using ripgrep and BSD sed
rg -l 'tenacious' | while IFS= read -r file; do sed -i '' 's/tenacious/relentless/g' "$file"; done

# Replace all instances of 'TENACIOUS' with 'RELENTLESS' using ripgrep and BSD sed
rg -l 'TENACIOUS' | while IFS= read -r file; do sed -i '' 's/TENACIOUS/RELENTLESS/g' "$file"; done
