"""Small VT screen reader for the fixture's Ratatui output (no external packages)."""
import re
import unicodedata


def screen_text(raw, width=120, height=32):
    cells = [[' '] * width for _ in range(height)]
    x = y = 0
    text = bytes(raw).decode('utf-8', errors='replace')
    i = 0
    while i < len(text):
        c = text[i]
        if c == '\x1b':
            match = re.match(r'\x1b\[([0-?]*)([ -/]*)([@-~])', text[i:])
            if match:
                params, _, op = match.groups()
                n = [int(p) if p.isdigit() else 0 for p in params.split(';')]
                if not params.startswith(('?', '>')):
                    v = n[0] or 1
                    if op in ('H', 'f'):
                        y, x = v - 1, ((n[1] or 1) if len(n) > 1 else 1) - 1
                    elif op == 'G': x = v - 1
                    elif op == 'd': y = v - 1
                    elif op == 'A': y -= v
                    elif op == 'B': y += v
                    elif op == 'C': x += v
                    elif op == 'D': x -= v
                    elif op == 'J' and n[0] in (2, 3): cells = [[' '] * width for _ in range(height)]
                    elif op == 'K':
                        a, b = (0, width) if n[0] == 2 else ((0, x + 1) if n[0] == 1 else (x, width))
                        if 0 <= y < height:
                            for col in range(max(0, a), min(width, b)): cells[y][col] = ' '
                x, y = max(0, min(width - 1, x)), max(0, min(height - 1, y))
                i += len(match.group())
                continue
            if text[i:i+2] == '\x1b]':
                end = re.search(r'\x07|\x1b\\', text[i+2:])
                if end is None: break
                i += 2 + end.end()
                continue
            if i + 1 == len(text) or text[i+1] == '[': break  # Incomplete escape.
            i += 2
            continue
        if c == '\r': x = 0
        elif c == '\n': y = min(height - 1, y + 1)
        elif c == '\b': x = max(0, x - 1)
        elif c == '\t': x = min(width - 1, (x // 8 + 1) * 8)
        elif c >= ' ':
            if not unicodedata.combining(c):
                columns = 2 if unicodedata.east_asian_width(c) in ('W', 'F') else 1
                if 0 <= x < width and 0 <= y < height: cells[y][x] = c
                if columns == 2 and x + 1 < width: cells[y][x+1] = ''
                x += columns
            elif x > 0: cells[y][x-1] += c
        i += 1
    return '\n'.join(''.join(row).rstrip() for row in cells)
