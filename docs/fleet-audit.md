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

## Still open

- [ ] Animation timing the simulation should wait for. The original holds the first shot until the
      guns have opened (`madOpenGunsShip`), docking until a carrier's doors are open
      (`CarrierDockDoorCheck`), and a link until both animations end. The port has no animation clock
      in the simulation, so these play without holding anything up. Fixing it needs the `.mad`
      durations in the statics. It does not break lockstep, as every peer runs the same code.
- [ ] Unlink: the split animation and turn are skipped (`processUnlinkToDo`).
- [ ] The sounds and chatter of hive and ramming animations (`madLinkInUpdateMeshAnimations` plays
      them with the animation).
- [ ] Ships under the Naggarok's EMP (`rndShipUnderNaggarokEMP` 0x5704d0). Campaign only.
- [ ] Salvage messages in `mrSpecialTargetOrder` when nothing can be salvaged, if any are missing.
- [ ] A Beast player seen on screen: everything is tested without one (`a_beast_player_builds_the_whole_fleet`,
      the Beast screens in the `hwc-ui` tests), but the campaign never runs one, so the first real look
      comes with the skirmish setup.

Not gaps: `*_Save/_Load/_Fix/_PreFix` (the port saves differently), `MCV*` wrappers, and helper
functions the Rust inlines. The AI player for skirmish is multiplayer work and is left for then.
