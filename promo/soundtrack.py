"""
Voice2Type 宣传片配乐与音效合成（全部程序化生成，无第三方素材版权问题）。

用法：python3 soundtrack.py out/events.json out/soundtrack.wav
依赖：numpy、scipy

结构（与 film.html 时间轴对齐）：
  0 – 6.2    前奏：氛围铺底 + 上升音效
  6.2        Logo 冲击音，铺底与铃音展开
  10.6 – 56.6 主段：拨弦琶音 + 柔和底鼓 + 次低音（视频配音段加密）
  56.6 – 62.2 过门：鼓组退出，上升音效
  62.2 – 68  结尾：冲击音 + 长和弦收束
节拍：为让 6.2s 与 62.2s 两个冲击点都落在小节线上，取 22 小节 / 56s（≈94.3 BPM）。
"""
import json
import sys

import numpy as np
from scipy import signal

SR = 48000
rng = np.random.default_rng(20261003)

events_path = sys.argv[1] if len(sys.argv) > 1 else 'out/events.json'
out_path = sys.argv[2] if len(sys.argv) > 2 else 'out/soundtrack.wav'
with open(events_path, encoding='utf-8') as f:
    data = json.load(f)
DUR = float(data['duration'])
EVENTS = data['events']
N = int(DUR * SR) + SR * 3

BAR = 56.0 / 22.0
BEAT = BAR / 4
ANCHOR = 6.2  # 小节线锚点


def downbeats(t0, t1):
    k0 = int(np.ceil((t0 - ANCHOR) / BAR - 1e-9))
    k1 = int(np.floor((t1 - ANCHOR) / BAR + 1e-9))
    return [ANCHOR + k * BAR for k in range(k0, k1 + 1)]


def hz(m):
    return 440.0 * 2 ** ((m - 69) / 12)


def tvec(dur):
    return np.arange(int(dur * SR)) / SR


def add(bus, x, t0, gain=1.0, pan=0.0):
    """x 为单声道或 (n, 2)；等功率声像"""
    i0 = int(round(t0 * SR))
    if i0 >= len(bus):
        return
    if x.ndim == 1:
        l, r = np.cos((pan + 1) * np.pi / 4), np.sin((pan + 1) * np.pi / 4)
        x = np.stack([x * l, x * r], axis=1) * np.sqrt(2)
    if i0 < 0:
        x = x[-i0:]
        i0 = 0
    n = min(len(x), len(bus) - i0)
    bus[i0:i0 + n] += x[:n] * gain


def env_ar(n, a, r, sustain_len=None):
    t = np.arange(n) / SR
    e = np.minimum(1.0, t / max(a, 1e-4))
    if sustain_len is not None:
        rel = np.clip((t - sustain_len) / max(r, 1e-4), 0, 1)
        e *= (1 - rel) ** 2
    return e


def lowpass(x, fc, order=2):
    b, a = signal.butter(order, fc / (SR / 2), 'low')
    return signal.lfilter(b, a, x, axis=0)


def highpass(x, fc, order=2):
    b, a = signal.butter(order, fc / (SR / 2), 'high')
    return signal.lfilter(b, a, x, axis=0)


def bandpass(x, lo, hi, order=2):
    b, a = signal.butter(order, [lo / (SR / 2), hi / (SR / 2)], 'band')
    return signal.lfilter(b, a, x, axis=0)


music = np.zeros((N, 2))
sfx = np.zeros((N, 2))

# ---------------- 和声 ----------------
# (低音, 铺底音符)
CHORDS = [
    (38, [50, 54, 57, 61, 64]),   # Dmaj9
    (35, [47, 50, 54, 57, 61]),   # Bm9
    (31, [47, 50, 54, 57, 61]),   # Gmaj9(#11)
    (33, [52, 57, 59, 62, 64]),   # Asus
]


def chord_at(t):
    k = int(np.floor((t - ANCHOR) / (2 * BAR)))
    return CHORDS[k % 4]


# ---------------- 铺底 Pad ----------------
def pad_voice(m, dur, bright=1.0):
    t = tvec(dur)
    f = hz(m)
    x = np.zeros_like(t)
    for det in (-0.07, 0.07):
        ff = f * 2 ** (det / 12)
        ph = rng.uniform(0, 2 * np.pi)
        for k in range(1, 9):
            x += np.sin(2 * np.pi * ff * k * t + ph * k) / (k ** (1.9 - 0.4 * bright))
    lfo = 1 + 0.12 * np.sin(2 * np.pi * 0.23 * t + rng.uniform(0, 6))
    return x * lfo


pad = np.zeros((N, 2))
chord_starts = [ANCHOR + k * 2 * BAR for k in range(-3, 13)]
for cs in chord_starts:
    if cs > DUR:
        break
    seg = 2 * BAR
    t0 = max(0.0, cs)
    length = seg + 2.2
    _, notes = chord_at(cs + 0.01)
    if cs < ANCHOR:
        notes = CHORDS[0][1]  # 前奏保持主和弦
    for j, m in enumerate(notes):
        v = pad_voice(m, length)
        e = env_ar(len(v), 1.4, 2.0, sustain_len=seg - (t0 - cs))
        add(pad, v * e, t0, gain=0.05, pan=(j - 2) * 0.28)
pad = lowpass(pad, 2600)
# 段落动态：前奏渐入、主段稳定、结尾长收束
t_axis = np.arange(N) / SR
pad_gain = np.interp(t_axis, [0, 0.4, 5.8, 6.2, 56.6, 60.0, 62.2, 66.0, DUR], [0, 0.35, 0.6, 1.0, 0.85, 0.75, 1.0, 0.7, 0])
music += pad * pad_gain[:, None]

# 前奏高频微光（声波意象）
shimmer_t = tvec(6.4)
shimmer = np.zeros_like(shimmer_t)
for m in (86, 90, 93, 97):
    shimmer += np.sin(2 * np.pi * hz(m) * shimmer_t + rng.uniform(0, 6)) * (0.5 + 0.5 * np.sin(2 * np.pi * rng.uniform(0.3, 0.9) * shimmer_t))
shimmer *= np.interp(shimmer_t, [0, 1.0, 2.5, 5.5, 6.4], [0, 0.6, 1.0, 0.8, 0])
add(music, shimmer, 0.0, gain=0.012)


# ---------------- 拨弦琶音 ----------------
def pluck(m, dur=0.7, tone=1.0):
    t = tvec(dur)
    f = hz(m)
    x = np.sin(2 * np.pi * f * t) + 0.45 * tone * np.sin(2 * np.pi * 2 * f * t) + 0.18 * tone * np.sin(2 * np.pi * 3.01 * f * t)
    e = np.minimum(1, t / 0.004) * np.exp(-t / 0.22)
    return x * e


def arp(t_from, t_to, step, gain, tone=1.0):
    t = t_from
    i = 0
    while t < t_to:
        _, notes = chord_at(t + 0.001)
        seq = notes + [n + 12 for n in notes[1:4]]
        order = seq + seq[-2:0:-1]
        m = order[i % len(order)] + 12
        accent = 1.0 if i % 4 == 0 else 0.72
        add(music, pluck(m, tone=tone), t, gain=gain * accent, pan=0.35 * np.sin(i * 0.9))
        t += step
        i += 1


first_beat = downbeats(10.0, 11.5)[0]
arp(first_beat, 56.6, BEAT / 2, 0.05)
arp(45.2 - ((45.2 - ANCHOR) % (BEAT / 4)), 56.6, BEAT / 4, 0.022, tone=0.6)  # 视频配音段加密
arp(56.6, 62.0, BEAT / 4, 0.026, tone=0.4)


# ---------------- 铃音 ----------------
def bell(m, dur=2.4):
    t = tvec(dur)
    f = hz(m)
    parts = [(1.0, 1.0, 1.6), (2.0, 0.5, 1.0), (3.01, 0.32, 0.7), (4.17, 0.2, 0.5), (5.43, 0.12, 0.35)]
    x = sum(a * np.sin(2 * np.pi * f * r * t) * np.exp(-t / d) for r, a, d in parts)
    return x * np.minimum(1, t / 0.002)


for k, m in enumerate([74, 78, 81, 85, 81, 78]):
    add(music, bell(m), ANCHOR + 0.1 + k * BEAT, gain=0.03, pan=-0.4 + 0.16 * k)
for k, m in enumerate([62, 69, 74, 78, 81, 85]):
    add(music, bell(m, 3.5), 62.2 + 0.15 + k * BEAT * 0.75, gain=0.028, pan=0.4 - 0.16 * k)


# ---------------- 鼓组 ----------------
def kick():
    t = tvec(0.6)
    f = 44 + 80 * np.exp(-t / 0.035)
    ph = 2 * np.pi * np.cumsum(f) / SR
    x = np.sin(ph) * np.exp(-t / 0.3)
    click = highpass(rng.standard_normal(len(t)), 2000) * np.exp(-t / 0.004) * 0.15
    return x + click


def shaker():
    t = tvec(0.09)
    return highpass(rng.standard_normal(len(t)), 6500) * np.exp(-t / 0.025)


K = kick()
kick_times = []
for db in downbeats(10.6, 56.6):
    beats = [0, 2] if db < 45.0 else [0, 1, 2, 3]
    for b in beats:
        tk = db + b * BEAT
        if 10.9 <= tk < 56.4:
            kick_times.append(tk)
            add(music, K, tk, gain=0.22 if b in (0, 2) else 0.14)
for db in downbeats(22.0, 56.6):
    for b in range(4):
        ts = db + b * BEAT + BEAT / 2
        if 22.6 <= ts < 56.4:
            add(music, shaker(), ts, gain=0.035, pan=0.5 if b % 2 else -0.5)

# 次低音（随底鼓轻微闪避）
sub = np.zeros(N)
for db in downbeats(10.6, 62.0):
    root, _ = chord_at(db + 0.001)
    t = tvec(BAR)
    v = np.sin(2 * np.pi * hz(root) * t) * env_ar(len(t), 0.05, 0.3, sustain_len=BAR - 0.3)
    i0 = int(db * SR)
    sub[i0:i0 + len(v)] += v[:max(0, min(len(v), N - i0))]
duck = np.ones(N)
for tk in kick_times:
    i0 = int(tk * SR)
    d = 1 - 0.65 * np.exp(-np.arange(int(0.3 * SR)) / (0.09 * SR))
    duck[i0:i0 + len(d)] = np.minimum(duck[i0:i0 + len(d)], d[:max(0, min(len(d), N - i0))])
sub_gain = np.interp(t_axis, [0, 10.6, 11.5, 56.0, 57.5, 62.2, 64.0, 67.0], [0, 0, 1, 1, 0.5, 0.9, 0.6, 0])
music += np.stack([sub * duck * sub_gain] * 2, axis=1) * 0.11
pad_duck = np.interp(t_axis, np.arange(N) / SR, duck) * 0.25 + 0.75
music *= pad_duck[:, None]


# ---------------- 音效 ----------------
def noise(dur):
    return rng.standard_normal(int(dur * SR))


def s_whoosh():
    d = 0.9
    t = tvec(d)
    n = bandpass(noise(d), 300, 6000)
    e = np.sin(np.pi * np.clip(t / d, 0, 1)) ** 2.2
    x = n * e
    pan = np.linspace(-0.8, 0.8, len(t))
    l, r = np.cos((pan + 1) * np.pi / 4), np.sin((pan + 1) * np.pi / 4)
    return np.stack([x * l, x * r], axis=1) * 0.5


def s_riser():
    d = 1.4
    t = tvec(d)
    n = highpass(noise(d), 1500)
    sweep = np.sin(2 * np.pi * np.cumsum(300 + 1500 * (t / d) ** 2) / SR) * 0.3
    e = (t / d) ** 2.5
    return (n * 0.5 + sweep) * e


def s_impact():
    d = 2.6
    t = tvec(d)
    f = 34 + 40 * np.exp(-t / 0.08)
    boom = np.sin(2 * np.pi * np.cumsum(f) / SR) * np.exp(-t / 0.9)
    air = lowpass(noise(d), 1800) * np.exp(-t / 0.35) * 0.4
    return boom + air


def s_type():
    t = tvec(0.03)
    return highpass(noise(0.03), 3000) * np.exp(-t / 0.004) * 0.6 + np.sin(2 * np.pi * 2600 * t) * np.exp(-t / 0.006) * 0.2


def s_keydown():
    t = tvec(0.14)
    thock = np.sin(2 * np.pi * (150 + 120 * np.exp(-t / 0.01)) * t) * np.exp(-t / 0.04)
    clack = bandpass(noise(0.14), 800, 4000) * np.exp(-t / 0.012) * 0.5
    return thock + clack


def s_keyup():
    t = tvec(0.08)
    return bandpass(noise(0.08), 1500, 6000) * np.exp(-t / 0.008) * 0.6


def s_click():
    t = tvec(0.05)
    return np.sin(2 * np.pi * 1700 * t) * np.exp(-t / 0.008) * 0.6 + highpass(noise(0.05), 4000) * np.exp(-t / 0.003) * 0.3


def s_pop():
    t = tvec(0.12)
    f = 520 + 520 * (t / 0.12)
    return np.sin(2 * np.pi * np.cumsum(f) / SR) * np.exp(-t / 0.035) * 0.7


def s_blip():
    t = tvec(0.25)
    a = np.sin(2 * np.pi * hz(81) * t) * np.exp(-t / 0.05)
    b = np.zeros_like(t)
    i = int(0.07 * SR)
    b[i:] = np.sin(2 * np.pi * hz(88) * t[:-i]) * np.exp(-t[:-i] / 0.07)
    return (a + b) * 0.5


def s_success():
    x = np.zeros(int(1.4 * SR))
    for k, m in enumerate([86, 90, 93]):
        b = bell(m, 1.2)
        i = int(k * 0.07 * SR)
        x[i:i + len(b)] += b[:len(x) - i]
    return x * 0.5


def s_send():
    t = tvec(0.35)
    f = 400 + 1600 * (t / 0.35) ** 1.5
    return (np.sin(2 * np.pi * np.cumsum(f) / SR) * 0.3 + bandpass(noise(0.35), 1500, 8000) * 0.4) * np.sin(np.pi * t / 0.35) ** 2


def s_tick():
    t = tvec(0.1)
    return np.sin(2 * np.pi * 1250 * t) * np.exp(-t / 0.02) * 0.5 + np.sin(2 * np.pi * 2500 * t) * np.exp(-t / 0.01) * 0.2


SFX = {
    'whoosh': (s_whoosh, 0.5, -0.35),
    'riser': (s_riser, 0.16, -1.4),
    'impact': (s_impact, 0.5, 0.0),
    'type': (s_type, 0.09, 0.0),
    'keydown': (s_keydown, 0.4, 0.0),
    'keyup': (s_keyup, 0.25, 0.0),
    'click': (s_click, 0.2, 0.0),
    'pop': (s_pop, 0.12, 0.0),
    'blip': (s_blip, 0.12, 0.0),
    'success': (s_success, 0.12, 0.0),
    'send': (s_send, 0.25, 0.0),
    'tick': (s_tick, 0.14, 0.0),
}
for i, e in enumerate(EVENTS):
    fn, g, offset = SFX.get(e['type'], (None, 0, 0))
    if fn is None:
        continue
    x = fn()
    pan = 0.0 if x.ndim == 2 else float(np.clip(np.sin(i * 1.7) * 0.25, -0.3, 0.3))
    add(sfx, x, e['t'] + offset, gain=g * e.get('gain', 1.0), pan=pan)


# ---------------- 混响 ----------------
def reverb_ir(length=2.6, decay=0.75):
    t = tvec(length)
    ir = np.stack([rng.standard_normal(len(t)), rng.standard_normal(len(t))], axis=1)
    ir *= np.exp(-t / decay)[:, None]
    ir = lowpass(ir, 5000)
    ir[: int(0.012 * SR)] = 0  # 预延迟
    return ir / np.sqrt(np.sum(ir ** 2, axis=0))


IR = reverb_ir()


def reverb(x, wet):
    out = np.zeros_like(x)
    for ch in range(2):
        out[:, ch] = signal.fftconvolve(x[:, ch], IR[:, ch])[: len(x)]
    return x + out * wet


music = reverb(music, 0.35)
sfx = reverb(sfx, 0.18)

mix = music * 0.9 + sfx
mix = mix[: int(DUR * SR)]
fade = np.ones(len(mix))
fi = int(0.05 * SR)
fade[:fi] = np.linspace(0, 1, fi)
fo = int(1.6 * SR)
fade[-fo:] = np.linspace(1, 0, fo) ** 1.5
mix *= fade[:, None]

# 母带：响度对齐（约 -16 dBFS RMS）+ 软限幅 + 峰值 -1 dBFS
rms = np.sqrt(np.mean(mix ** 2))
mix *= 10 ** (-16 / 20) / max(rms, 1e-9)
mix = np.tanh(mix * 1.1) / np.tanh(1.1)
mix *= 10 ** (-1 / 20) / max(np.max(np.abs(mix)), 1e-9)

pcm = (mix * 32767).astype(np.int16)
import wave  # noqa: E402

with wave.open(out_path, 'wb') as w:
    w.setnchannels(2)
    w.setsampwidth(2)
    w.setframerate(SR)
    w.writeframes(pcm.tobytes())
print(f'soundtrack -> {out_path}  ({DUR:.1f}s, rms {20 * np.log10(np.sqrt(np.mean(mix ** 2))):.1f} dBFS)')
