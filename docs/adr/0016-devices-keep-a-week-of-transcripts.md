---
status: accepted
---

# Every device keeps the last week of its Vaults' Transcripts and can refill an erased Host

ADR 0008 left the Transcripts only on the Vault's Host, so a Host that lost its disk took
every Conversation with it, and the records' copies on the devices could not refill it.

Each Mac and phone now also keeps the Transcripts its Vault's Host changed in the last seven
days, newest first, up to 500 MB per Vault, as Telegram caps its cache by age and by size. A
Transcript only grows, so the device fetches just the bytes past its own copy's length, on the
same 30-second round that copies Transcripts into the Vault. Each copy carries the Host's time
for the file, and the device deletes copies by those times alone, never by the Host's list, so
an empty Host cannot wipe them. Older Transcripts are archived by hand.

A Host whose latest change is older than the one a device has seen has lost its database. The
device then queues every record the Host lacks, appends its Transcripts to the Host's shorter
copies, and reads the Vault again from the start.

## Considered options

- A nightly rsync of each Vault to another machine. It needs no app change, but Macs and
  phones sleep at night. Rejected.
- Every Transcript on every device. The hobby Vault's come to 1.1 GB. Rejected.

## Consequences

A restored Host lacks every Transcript older than a week unless it is brought back from the
hand-made archive. When two devices refill the same Host, the second finds the records there
and adds only what is missing.
