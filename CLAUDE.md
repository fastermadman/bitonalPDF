# bitonalPDF: working agreements

## Plain-language summary (owner is not a specialist in this code)

Every session that finishes a piece of work (PR opened, issue closed, investigation ended) ends its final report with a block
headed **"Kort fortalt"**, in plain Danish or English, 3-5 lines, no internal jargon (no "band rule", "plan box", "G4", issue
numbers without saying what they are):

1. What was the problem, as the end user would see it?
2. What did we change, in one sentence?
3. What the owner must judge or decide (if anything), and what is just FYI.
4. Is it safe to merge, and what is the sensible next step?

A session that starts work on an issue opens with the same kind of 2-3 lines: what this issue is for, and whether the result
will be visible in the output PDFs at all.

**The summary is added, never a substitute.** Everything else stays as strict as before: verbatim numbers (not paraphrased),
every metric in the issue checked and reported, "if a number differs, stop and report", known limits and anything that got worse
listed explicitly. The block must not hide a failed check or a "needs a look"; if one exists it goes in "Kort fortalt" too.
