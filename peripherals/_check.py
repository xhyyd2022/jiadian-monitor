"""peripherals JSON 语义参照实现：用来在写 Rust 前验证 sample 回放是否自洽。

只实现协议库用到的子集：表达式求值、tx 组帧、rx 匹配、sample 回放。
"""
import json, os, re, sys

ROOT = os.path.dirname(os.path.abspath(__file__))


def hexbytes(s):
    s = re.sub(r'[^0-9a-fA-F]', '', s or '')
    if len(s) % 2:
        raise ValueError('odd hex: ' + s)
    return [int(s[i:i + 2], 16) for i in range(0, len(s), 2)]


TOKEN = re.compile(r'\s*(0x[0-9a-fA-F]+|\d+\.\d+|\d+|b\[\d+\]|[A-Za-z_]\w*|==|!=|<=|>=|<<|>>|&&|\|\||[-+*/%&|^~!<>()\[\],])')


def tokenize(src):
    out, i = [], 0
    while i < len(src):
        m = TOKEN.match(src, i)
        if not m:
            raise ValueError('bad token at %d in %r' % (i, src))
        out.append(m.group(1))
        i = m.end()
    return out


class Ev:
    def __init__(self, buf):
        self.b = buf

    # --- helpers used by function calls
    def u16le(self, o):
        return self._at(o) | (self._at(o + 1) << 8)

    def u16be(self, o):
        return (self._at(o) << 8) | self._at(o + 1)

    def s16le(self, o):
        v = self.u16le(o)
        return v - 0x10000 if v & 0x8000 else v

    def s8(self, o):
        v = self._at(o)
        return v - 0x100 if v & 0x80 else v

    def _at(self, o):
        if o < 0 or o >= len(self.b):
            raise IndexError('offset %d out of range (len=%d)' % (o, len(self.b)))
        return self.b[o]

    def _sum(self, a, bnd):
        return sum(self._at(i) for i in range(a, min(bnd, len(self.b) - 1) + 1))

    def _xor(self, a, bnd):
        v = 0
        for i in range(a, min(bnd, len(self.b) - 1) + 1):
            v ^= self._at(i)
        return v

    def eval(self, src):
        self.t = tokenize(src)
        self.i = 0
        v = self._expr()
        if self.i != len(self.t):
            raise ValueError('trailing tokens: %r' % self.t[self.i:])
        return v

    def _peek(self):
        return self.t[self.i] if self.i < len(self.t) else None

    def _next(self):
        v = self.t[self.i]
        self.i += 1
        return v

    def _expr(self):
        return self._or()

    def _or(self):
        v = self._and()
        while self._peek() == '||':
            self._next()
            r = self._and()
            v = 1 if (v or r) else 0
        return v

    def _and(self):
        v = self._bor()
        while self._peek() == '&&':
            self._next()
            r = self._bor()
            v = 1 if (v and r) else 0
        return v

    def _bor(self):
        v = self._bxor()
        while self._peek() == '|':
            self._next()
            v = int(v) | int(self._bxor())
        return v

    def _bxor(self):
        v = self._band()
        while self._peek() == '^':
            self._next()
            v = int(v) ^ int(self._band())
        return v

    def _band(self):
        v = self._cmp()
        while self._peek() == '&':
            self._next()
            v = int(v) & int(self._cmp())
        return v

    def _cmp(self):
        v = self._add()
        while self._peek() in ('==', '!=', '<', '<=', '>', '>='):
            op = self._next()
            r = self._add()
            v = int({'==': v == r, '!=': v != r, '<': v < r, '<=': v <= r, '>': v > r, '>=': v >= r}[op])
        return v

    def _add(self):
        v = self._mul()
        while self._peek() in ('+', '-'):
            op = self._next()
            r = self._mul()
            v = v + r if op == '+' else v - r
        return v

    def _mul(self):
        v = self._unary()
        while self._peek() in ('*', '/', '%'):
            op = self._next()
            r = self._unary()
            if op == '*':
                v = v * r
            elif op == '/':
                v = v / r if r else 0
            else:
                v = v % r if r else 0
        return v

    def _unary(self):
        t = self._peek()
        if t == '-':
            self._next()
            return -self._unary()
        if t == '+':
            self._next()
            return self._unary()
        if t == '!':
            self._next()
            return 0 if self._unary() else 1
        if t == '~':
            self._next()
            return ~int(self._unary()) & 0xFFFFFFFF
        return self._shift()

    def _shift(self):
        v = self._atom()
        while self._peek() in ('<<', '>>'):
            op = self._next()
            r = int(self._atom())
            v = int(v) << r if op == '<<' else int(v) >> r
        return v

    def _atom(self):
        t = self._next()
        if t == '(':
            v = self._expr()
            assert self._next() == ')'
            return v
        if t.startswith('0x') or t.startswith('0X'):
            return int(t, 16)
        if re.fullmatch(r'\d+\.\d+', t):
            return float(t)
        if re.fullmatch(r'\d+', t):
            return int(t)
        m = re.fullmatch(r'b\[(\d+)\]', t)
        if m:
            return self._at(int(m.group(1)))
        if re.fullmatch(r'[A-Za-z_]\w*', t):
            args = []
            if self._peek() == '(':
                self._next()
                if self._peek() != ')':
                    args.append(self._expr())
                    while self._peek() == ',':
                        self._next()
                        args.append(self._expr())
                assert self._next() == ')'
            return self._call(t, args)
        raise ValueError('unexpected token %r' % t)

    def _call(self, name, a):
        if name == 'u16le':
            return self.u16le(int(a[0]))
        if name == 'u16be':
            return self.u16be(int(a[0]))
        if name == 's16le':
            return self.s16le(int(a[0]))
        if name == 's8':
            return self.s8(int(a[0]))
        if name == 'min':
            return min(a)
        if name == 'max':
            return max(a)
        if name == 'abs':
            return abs(a[0])
        if name == 'round':
            return int(round(a[0]))
        if name == 'clamp':
            return max(a[1], min(a[2], a[0]))
        if name == 'if':
            return a[1] if a[0] else a[2]
        if name == 'sum':
            return self._sum(int(a[0]), int(a[1]))
        if name == 'xor':
            return self._xor(int(a[0]), int(a[1]))
        if name == 'len':
            return len(self.b)
        raise ValueError('unknown func ' + name)


def build_tx(frame):
    n = int(frame['length'])
    pad = int(str(frame.get('pad', '00')), 16)
    buf = [pad] * n
    rid = frame.get('reportId')
    if rid is not None:
        buf[0] = int(str(rid), 16)
    for k, v in (frame.get('bytes') or {}).items():
        o = int(k)
        if o >= n:
            raise ValueError('tx offset %d >= length %d' % (o, n))
        buf[o] = int(str(v), 16)
    ck = frame.get('checksum')
    if ck:
        p = int(ck['put'])
        if p >= n:
            raise ValueError('checksum put %d >= length %d' % (p, n))
        buf[p] = int(Ev(buf).eval(ck['expr'])) & 0xFF
    return buf


def rx_ok(rx, buf):
    for m in rx.get('match') or []:
        o = int(m['offset'])
        if o >= len(buf) or buf[o] != int(str(m['equals']), 16):
            return 'nomatch'
    for m in rx.get('skip') or []:
        o = int(m['offset'])
        if o < len(buf) and buf[o] == int(str(m['equals']), 16):
            return 'skip'
    return None


def num_field(spec, buf):
    ev = Ev(buf)
    if 'expr' not in spec:
        return None
    v = ev.eval(spec['expr'])
    if spec.get('round', True):
        v = int(round(v))
    lo = spec.get('min', 0)
    hi = spec.get('max', 100)
    if not (lo <= v <= hi):
        raise ValueError('out of range %s not in [%s,%s]' % (v, lo, hi))
    return v


def main():
    idx = json.load(open(os.path.join(ROOT, 'index.json'), encoding='utf-8'))
    fails, total = [], 0
    for key, bmeta in idx['brands'].items():
        bd = json.load(open(os.path.join(ROOT, bmeta['file']), encoding='utf-8'))
        for pname, p in bd['protocols'].items():
            smp = p.get('sample')
            if not smp:
                continue
            total += 1
            tag = '%s/%s' % (key, pname)
            try:
                buf = hexbytes(smp['rx'])
            except Exception as e:
                fails.append('%s: bad hex %s' % (tag, e))
                continue
            rx = p.get('rx') or {}
            st = rx_ok(rx, buf) if rx else None
            if st == 'skip':
                fails.append('%s: sample 被 rx.skip 丢弃' % tag)
                continue
            if st == 'nomatch':
                fails.append('%s: sample 不满足 rx.match' % tag)
                continue
            exp = smp.get('expect') or {}
            if 'battery' in exp:
                try:
                    got = num_field(p['battery'], buf)
                except Exception as e:
                    fails.append('%s: battery 解析失败 %s' % (tag, e))
                    continue
                if got != exp['battery']:
                    fails.append('%s: battery 期望 %s 实得 %s' % (tag, exp['battery'], got))
            for f in p.get('fields') or []:
                nm = f['name']
                if nm not in exp:
                    continue
                try:
                    if 'ascii' in f:
                        o, n = f['ascii']['offset'], f['ascii']['length']
                        got = bytes(buf[o:o + n]).decode('ascii', 'replace').rstrip('\x00')
                    else:
                        got = num_field(f, buf)
                except Exception as e:
                    fails.append('%s: field %s 解析失败 %s' % (tag, nm, e))
                    continue
                if got != exp[nm]:
                    fails.append('%s: field %s 期望 %r 实得 %r' % (tag, nm, exp[nm], got))
            # tx 组帧也要能成功
            txs = p.get('tx')
            if isinstance(txs, dict):
                txs = [txs]
            for fr in txs or []:
                try:
                    build_tx(fr)
                except Exception as e:
                    fails.append('%s: tx 组帧失败 %s' % (tag, e))
            for pre in p.get('pre') or []:
                try:
                    build_tx(pre['tx'])
                except Exception as e:
                    fails.append('%s: pre 组帧失败 %s' % (tag, e))
    print('samples:', total)
    if fails:
        print('FAILURES:')
        for f in fails:
            print('  -', f)
        return 1
    print('all samples pass')
    return 0


if __name__ == '__main__':
    sys.exit(main())
