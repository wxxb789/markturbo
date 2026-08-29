# Measure The Release Profile On A Quiet Host

Compare the current release profile with an `opt-level = "s"` candidate using
low-overhead CPU and disk quietness checks and an A-B-B-A sequence for startup
and first-formula measurements.

Do not adopt the smaller profile from binary size alone. If the machine does not
meet the pre-registered quietness gate, record the runtime comparison as deferred
and leave the release profile unchanged.
