#!/usr/bin/env python3
"""Replicate the Rust representative_corpus() gate corpus in Python.

The postcard layouts were verified byte-for-byte against /tmp/boru_corpus
dumps (see gen_dictionary_and_analyze).  Text messages dominate the gate
corpus, exactly as in the Rust test.  Deterministic hashes stand in for
blake3/ed25519 randomness (incompressible either way).

Usage:
  python3 scripts/replicate_gate_corpus.py   # measure current DICTIONARY vs gate replica
"""
import hashlib
import re
import sys

sys.path.insert(0, 'scripts')
from tune_dictionary import load_current_dictionary, raw_deflate_with_dict


def varint(n):
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def b32(s):
    return hashlib.blake2b(s.encode(), digest_size=32).digest()


def ticket(s):
    return b"blob:iroh:" + hashlib.blake2b(s.encode(), digest_size=32).hexdigest().encode() + b":3:200:1000"


def str_field(s):
    b = s.encode() if isinstance(s, str) else s
    return varint(len(b)) + b


def opt_str(v):
    if v is None:
        return b"\x00"
    return b"\x01" + str_field(v)


def opt_bytes(v):
    if v is None:
        return b"\x00"
    return b"\x01" + v


def encode_message(m):
    """m is (variant, fields...) following the Message enum order."""
    kind = m[0]
    if kind == "Message":
        return b"\x01" + str_field(m[1])
    if kind == "AboutMe":
        return b"\x00" + str_field(m[1]) + opt_str(m[2])
    if kind == "FileShare":
        return b"\x02" + str_field(m[1]) + str_field(m[2]) + varint(m[3]) + opt_bytes(m[4])
    if kind == "Leave":
        return b"\x03"
    if kind == "Presence":
        return b"\x04"
    if kind == "PresenceWithTicket":
        return b"\x05" + str_field(m[1])
    if kind == "ReadReceipt":
        return b"\x06" + m[1]
    if kind == "Edit":
        return b"\x07" + m[1] + str_field(m[2])
    if kind == "Delete":
        return b"\x08" + m[1]
    if kind == "Reaction":
        return b"\x09" + m[1] + str_field(m[2])
    if kind == "ImageShare":
        return b"\x0a" + str_field(m[1]) + m[2]
    if kind == "Heartbeat":
        return b"\x0c"
    if kind == "LatencyPing":
        return b"\x0d" + varint(m[1])
    if kind == "LatencyPong":
        return b"\x0e" + varint(m[1])
    raise ValueError(kind)


def room_advertisement(room_name, description, topic_hash, ticket_str, member_count, last_activity, sig):
    out = b"\x0b"
    out += str_field(room_name)
    out += str_field(description)
    out += topic_hash
    out += str_field(ticket_str)
    out += varint(member_count)
    out += varint(last_activity)
    out += varint(len(sig)) + sig
    return out


def diagnostic_probe(probe_id, sender_id, room_id, sent_at_ms, payload):
    out = b"\x0f"
    out += str_field(probe_id)
    out += str_field(sender_id)
    out += str_field(room_id)
    out += varint(sent_at_ms)
    out += opt_str(payload)
    return out


def contact_control(payload):
    return b"\x10" + varint(len(payload)) + payload


def profile_update(user_id, display_name, bio, avatar_identifier, shared_folder_path,
                   file_sharing_enabled, allow_downloads, max_file_size,
                   allowed_extensions, shared_files):
    out = b"\x11"
    out += user_id
    out += str_field(display_name)
    out += str_field(bio)
    out += opt_str(avatar_identifier)
    out += str_field(shared_folder_path)
    out += b"\x01" if file_sharing_enabled else b"\x00"
    out += b"\x01" if allow_downloads else b"\x00"
    out += varint(max_file_size)
    out += varint(len(allowed_extensions))
    for ext in allowed_extensions:
        out += str_field(ext)
    out += varint(len(shared_files))
    for sf in shared_files:
        # SharedFileMeta { id, filename, size, mime_type, modified_time, hash }
        out += str_field(sf[0]) + str_field(sf[1]) + varint(sf[2]) + str_field(sf[3]) + varint(sf[4]) + sf[5]
    return out


def encrypted_group_message(group_id, envelope):
    return b"\x12" + group_id + envelope


def build_gate_corpus():
    corpus = []
    texts = [
        "hi there",
        "just checking in, how is everyone doing today",
        "I was looking at the new update we shipped last night and it looks really good",
        "can you send me the files when you get a chance, no rush at all",
        "the weather is finally getting better, maybe we should go for a walk this weekend",
        "did you see the message I posted earlier about the meeting tomorrow morning",
        "ok sounds good, I will take a look and get back to you in a bit",
        "thanks for the help, I really appreciate it",
        "yeah that makes sense, I think we should try it and see how it goes",
        "I'm almost done with the report, just need to finish the last section",
        "remind me to pick up some groceries on the way home and call the plumber about the kitchen sink",
        "the new version is out now, it fixes the connection issues we were having and adds a dark theme",
        "I posted the slides from yesterday's talk in the shared folder, let me know if you can open them",
        "we should plan the trip soon, I was thinking about going to the mountains next month",
        "that sounds perfect, I am free on Saturday afternoon if you want to meet up and grab some coffee",
        "the server was down for a few hours this morning but everything is back online now",
        "could you double check the numbers in the spreadsheet before we send it to the client",
        "I heard they are working on a new feature that lets you share files without any size limit",
        "sure, I can help with that, just let me know what you need and I will get back to you as soon as I can",
        "we should really get together some time and catch up, it has been way too long since we last talked",
        "the download finished and everything works now, thanks again for pointing me to the right place",
        "I'm heading out for a bit, if anything comes up just leave a message and I will see it when I get back",
        "that sounds like a great idea, let's do it, I will check my schedule and let you know what works best for me",
        "the meeting has been moved to tomorrow morning at ten, could you let everyone in the group know about the change",
        "I've been thinking about what you said earlier and I think you are right, we should go with your plan",
        "no worries at all, these things happen, we can figure it out together when you have a moment to look at it",
        "hey everyone, welcome to the group, feel free to introduce yourselves",
        "I just sent you a friend request, check your requests when you have a moment",
        "can we move the call to later this afternoon, I have a meeting at two",
        "the link you shared earlier is not working for me, could you send it again",
        "I really like the new design, it looks much cleaner than the old one",
        "let me know when you are free and we can figure out a good time to talk",
        "thanks for sending that over, I will take a look tonight and get back to you",
        "did anyone else have trouble connecting this morning, the server seemed slow",
        "I will be away for the next few days, so I might not reply right away",
        "sounds good to me, just send me the details and I will add it to my calendar",
        "that's a really good point, I hadn't thought about it that way before",
        "we should have a quick chat about the project before the end of the week",
        "the file you uploaded is too large, try compressing it or splitting it up",
        "I found the issue, it was a simple mistake in the config file, fixing it now",
        "are you coming to the party on Friday, it should be a lot of fun",
        "happy birthday, hope you have a great day and a wonderful year ahead",
        "let's grab lunch sometime this week, I know a good place near the office",
        "I'll be back online in about an hour, just finishing up some errands",
        "thanks everyone for the warm welcome, looking forward to chatting with you all",
        "can you remind me to call the doctor tomorrow morning, I always forget",
        "I think the new update broke something on my phone, the app keeps crashing",
        "no problem, take your time, there is no hurry at all",
        "we are meeting at the usual place on Saturday, bring your friends if you want",
        "could you check the settings and make sure the notifications are turned on",
        "hi, I just joined the group, nice to meet everyone here",
        "the video call should start in a few minutes, see you all there",
        "I will send the document right after lunch, it is almost ready",
        "thanks for waiting, I had to deal with something at work",
        "sure thing, I can help you set that up later today",
    ]
    for i, t in enumerate(texts):
        corpus.append((f"Message-{i}", encode_message(("Message", t))))

    corpus.append(("AboutMe", encode_message(("AboutMe", "carol", ticket("carol-avatar")))))
    corpus.append(("FileShare", encode_message(("FileShare", "holiday_photos.zip", ticket("holiday"), 88_000_000, b32("thumb")))))
    corpus.append(("Leave", encode_message(("Leave",))))
    corpus.append(("Presence", encode_message(("Presence",))))
    corpus.append(("Heartbeat", encode_message(("Heartbeat",))))
    corpus.append(("PresenceWithTicket", encode_message(("PresenceWithTicket", ticket("dave-room")))))
    corpus.append(("ReadReceipt", encode_message(("ReadReceipt", b32("rcpt-a")))))
    corpus.append(("Edit", encode_message(("Edit", b32("edit-a"), "rewritten message text"))))
    corpus.append(("Delete", encode_message(("Delete", b32("del-a")))))
    corpus.append(("Reaction", encode_message(("Reaction", b32("react-a"), "🔥"))))
    corpus.append(("ImageShare", encode_message(("ImageShare", "IMG_20260804_183000.jpg", b32("img-a")))))

    ad = ("Boru Dev Chat", "Discussion about boru development, networking and compression.",
          b32("boru-dev"), ticket("boru-dev"), 7, 1_723_100_000_000, bytes([0xcc]) * 64)
    corpus.append(("RoomAdvertisement", room_advertisement(*ad)))
    corpus.append(("LatencyPing", encode_message(("LatencyPing", 1_723_100_000_042))))
    corpus.append(("LatencyPong", encode_message(("LatencyPong", 1_723_100_000_042))))
    corpus.append(("DiagnosticProbe", diagnostic_probe(
        "probe-02J4BCD8L0ZYX9876543210",
        b32("key-a").hex().encode(), b32("boru-dev").hex().encode(),
        1_723_100_000_042, "route=direct,version=0.113.0,os=linux")))
    corpus.append(("ContactControl", contact_control(bytes([0xde]) * 128)))
    sf = [
        ("sf-9", "holiday_photos.zip", 88_000_000, "application/zip", 1_723_000_500_000, b32("holiday")),
    ]
    corpus.append(("ProfileUpdate", profile_update(
        b32("key-a"), "carol", "Frontend developer, into distributed systems and coffee.",
        ticket("carol-avatar"), "/home/carol/Documents/Boru/Shared",
        True, False, 512 * 1024 * 1024, ["jpeg", "webp"], sf)))
    corpus.append(("EncryptedGroupMessage", encrypted_group_message(
        b32("boru-dev"), b"\x10" + bytes([0xC3]) * 64 + b"\x0c\x01\x14another-direct-payload")))
    return corpus


def main():
    corpus = build_gate_corpus()
    d = load_current_dictionary()
    raw_total = sum(len(b) for _, b in corpus)
    comp_total = sum(len(raw_deflate_with_dict(b, d)) for _, b in corpus)
    print(f"gate replica: {len(corpus)} entries, current DICTIONARY {len(d)} bytes")
    print(f"TOTAL raw {raw_total} comp {comp_total} ratio {raw_total / comp_total:.2f}")
    for name, b in corpus:
        c = raw_deflate_with_dict(b, d)
        print(f"{name:<22} raw {len(b):>5}  comp {len(c):>5}  ratio {len(b)/len(c):>5.2f}")


if __name__ == '__main__':
    main()
