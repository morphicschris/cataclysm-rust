# Somtaaw and Beast fleet audit

Multiplayer lets a player field any Somtaaw (`race_sect`) or Beast (`race_beast`) ship, including
the ones the campaign never gives the player, so every type, ability and system has to work before
multiplayer starts. This is the audit of the port against `cataclysm.exe`, and the list of what is
left. Tick items off here as they land.

How it was checked:

* the per-type `CustShipHeader` table at 0x8a75f4 (indexed by ship type), every callback slot of
  every Somtaaw and Beast type, against `hwc_sim::ships`;
* every function of the ship modules, the salvage code and the player orders, against what the Rust
  cites by address or name;
* every caller of `suShipHasUpgrade`, by upgrade bit;
* every key in the `.shp` files of the two races, against the keys the port reads;
* `crates/hwc-sim/tests/fleet_coverage.rs`: each type with all its upgrades, through attack,
  special, halt and dock. No type panics.

## Done

Simulation: the heavy corvette burst attack and `pointExplosionInSpace`, the Somtaaw destroyer's
missile regeneration, crystal harvesting (`salvDepotProcessCrystal`, `consume*`), the Beast and big
gun depot processes, `salvReleaseTarget`, `salvHouseKeep`, `salvCheckDocked`, the Dreadnought
cannon's reclamp.

Player control: Siege Cannon targeting and cancel, Cancel Swarm, the Beast mothership systems
buttons, module repair priorities, enter slipgate (key and cursor), the Beast voice set.

Presentation: the crystal's clip plane and beam, the retrofit phase blend, cloak fade, the animations
the ship modules start (hive wings, sentinels, ramming frigates, leech, carrier doors, the Siege
Cannon's recoil, ion array, Mimic link).

Checked and not gaps: bombers' bombing run (`GenericInterceptorAttack` can never reach it: it needs
a non-ship target of a ship class). The Beast carrier and the processors doing no damage head on in
`fleet_coverage` is the data: their guns point sideways, with cones that do not reach the nose. `trail2*`,
`scaleCap`, `badAI` and `fightmanAIP` in the `.shp` files are not read by the original either.

## Done in the second pass

- The animation clock is simulation state (`hwc_sim::mad`, `.mad` windows in the statics, save version
  7). What waits on it now waits as in the original: the ion array frigates' guns open before they
  fire and close after, a ship leaving a Somtaaw carrier's bay waits for the doors, a worker's jack
  and the Acolytes' pods run their real lengths. The view draws from it (`World`), so animations and
  orders cannot disagree. The animations' sounds come from `madAnimSoundTableInit`.
- Unlink is the full `processUnlinkToDo`: the turn, the two parts side by side playing the split, the
  second holding station until it ends, the roll, the health and kills since the link shared out, and
  the parts staying selected.
- The player's own cloaked ship draws lit without textures (`0x8f18b2`). The Naggarok under way
  crawls with bolts after `G_NaggarokEMP` (`rndShipMovingUnderNaggarokEMP`).
- The `mrSpecialTargetOrder` messages: what cannot be salvaged and why, "nothing found" with the
  ship to focus on, EMP recharging. `salvCanSalvageTarget` also refuses for want of a crystal
  processor or an infector, and for a type a Beast depot cannot convert.
- A Beast player on screen: `campaign_shots::beast_player_look` (`--ignored`) shows the fleet and its
  build, research and systems managers, all with the Beast's own lists and buttons.

## Done in the third pass (checked line by line against the original)

- A link merges when both link animations have ended (`madAnimation +0x14`), or after the 2 s
  `processLinkToDo` allows them: the Acolytes' 1.6 s animation now sets the pace. Test
  `fleet_coverage::acolytes_merge_when_their_link_animations_end`.
- The Naggarok's EMP look is not bolts: `rndShipMovingUnderNaggarokEMP` sets a render state that
  `meshCurrentMaterialDefault` turns into a material ambient of (v, v, 1), v bouncing between
  0.45 and 1, stepped once per drawn frame and by 0.1 more per material (bands across the hull).
  Ported as `ShipRenderState::emp_ambient` and the ship shader; the base steps per update rather
  than per frame, so the pulse keeps its pace at any frame rate. Shots
  `campaign_shots::fleet_looks` (33, 33b).
- The ramming frigate plays `0x81c` when it latches on (`RammingFrigateCheckBump`), besides the
  animation's own sound, and its first trail's ribbon is 1.3 times longer (4 with the ram upgrade)
  while it charges or pushes (`RammingFrigateSetState`, `ramming::trail_scale`). The hive's
  speech and chatter were already in.
- Another player's cloaked ship the local player cannot see (`univShipVisibleToPlayer`) is not
  drawn in the main view, nor are its trails (`rndRenderObject`, HW1's trail test). The player's
  own cloaked ship drawing untextured, and the fade, match.
- The Somtaaw carrier's doors (`CarrierHouseKeep`, `CarrierDockDoorCheck`, `CarrierInit`) were
  checked and are 1:1.
- The Beast player's build and research managers on a real skirmish level:
  `campaign_shots::skirmish_plays` (40a, 40b).

Found while checking, not ported: the per-ship part of `soundEventUpdate` (0x517e30: engine hums,
ambient, damage and the special loops in `SOUNDEVENT +0x10`, which `RammingFrigateSetState` stops
when it starts pushing) has no port, so no ship has engine sounds.

## Still open

Nothing on the list from the original audit. Two things are knowingly left:

- `MadSpecialCaseAnims` (gravity well generators and proximity sensors): those are Kushan and Taiidan
  ships, not in either fleet, and keep their own opening logic.

Not gaps: `*_Save/_Load/_Fix/_PreFix` (the port saves differently), `MCV*` wrappers, and helper
functions the Rust inlines. The AI player for skirmish is multiplayer work and is left for then.
