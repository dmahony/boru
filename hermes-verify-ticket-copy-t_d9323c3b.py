#!/usr/bin/env python3
"""Ad-hoc verification: ticket click-to-copy string matching logic."""

TICKET_PREFIX = "Ticket to join this room: "

def extract_ticket(text):
    if text.startswith(TICKET_PREFIX):
        return text[len(TICKET_PREFIX):]
    return None

# 1) Normal ticket line
ticket = "i3ljnkzgtycyyze6ss64dyaabyrqjupqd655pk3hlbhtomamairqcvx2loqwfmecj5ovww5wssd3yvvdba6ade57ir2pk7obz6gxgjebayacg2duoryhgorpf52xg5zrfuys44tfnrqxsltogaxgs4tpnaxgy2lonmxc6aiaswtqwnw7fmaqblaqab3zleydaeakyeiaagkzgaybacwbeaabswjqgaiaycuhuamvsmbq"
line = f"{TICKET_PREFIX}{ticket}"
result = extract_ticket(line)
assert result == ticket, f"FAIL"
print(f"PASS: ticket line -> copies ticket value ({len(ticket)} chars)")

# 2) Non-ticket system message
result = extract_ticket("Waiting for peers to join us...")
assert result is None
print("PASS: non-ticket system message ignored")

# 3) Local message
result = extract_ticket("[alice] hello")
assert result is None
print("PASS: local message ignored")

# 4) Remote message
result = extract_ticket("[bob] hi there")
assert result is None
print("PASS: remote message ignored")

# 5) Partial prefix
result = extract_ticket("Ticket to join")
assert result is None
print("PASS: partial prefix not matched")

# 6) Short ticket
result = extract_ticket(f"{TICKET_PREFIX}abc123")
assert result == "abc123"
print("PASS: short ticket value")

print()
print("All 6 assertions passed.")
