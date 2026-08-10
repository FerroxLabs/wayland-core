# #412 — my totals come out low, and it died on a blank line

opened by dana.k

Right, so I do my expenses by pasting the lines straight out of the bank export
into `receipts` and letting it add them up.

Two things.

The total is wrong. It's always a bit low. I worked out that it's the lines
where the bank puts a symbol in front of the number — most of mine come through
as a plain `12.50` but the travel card puts a `£` on and the German one puts a
`€` on, and I've got a couple of old `$` ones in there too. Those lines just
vanish. No warning, they're not in the total, nothing.

And it fell over completely this morning, big traceback, because I'd left a
blank line in the middle where I'd been tidying up. That shouldn't kill it, it's
a blank line.

The bit that actually bothers me is the silence. If there's a line it genuinely
can't make sense of — and there will be, the bank writes some odd stuff — I'd
much rather it told me which line and what it was, than quietly leave it out and
hand me a number I'll only find is wrong at the end of the quarter.
