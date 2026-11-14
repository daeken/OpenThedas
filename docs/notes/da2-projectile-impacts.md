# DA2 projectile impact replacement

The DA2 script declaration says `SetProjectileImpactEvent` replaces the projectile's
custom impact event. Read-only tracing confirms command 371's wrapper at 0x4eaa60 calls
0x4dbf10: it copies the event into one projectile member, at offset 0x404. This setter
has no flight-time calculation. The projectile destructor at 0x5e3d3f releases that member.

The simulator previously queued a new event for every call and delayed each by the
projectile's entire flight duration, measured from the setter. Replacing an impact halfway
through flight therefore produced two hits, with the replacement arriving late.

DA2 now retains the event-queue key per projectile. A replacement removes that projectile's
previous queued payload and keeps the arrival deadline recorded at launch. The mapping is
saved, so replacing a payload after reload behaves the same way. Other events for the same
recipient remain independent. The mapping is omitted when empty, preserving Origins
snapshot serialization; Origins retains its existing behavior in this scoped change.

A synthetic VM integration launches a one-second shot, saves/reloads halfway through,
and replaces its impact. Previously the two callbacks arrived at 1,000 and 1,500 ms;
now only the replacement arrives, at 1,000 ms. The regression also covers a large u64
world clock, recipient and creator, unrelated queued events, and invalid projectile objects.

This does not establish native flight speed, acceleration, collision, homing, launch
animation timing, or the default ability executor's projectile path. The existing fixed
speed remains provisional. Custom impact events already queued by older engine snapshots
have no ownership marker and are not retroactively identified or removed. Destruction
continues to use the existing end-of-tick cleanup policy.
