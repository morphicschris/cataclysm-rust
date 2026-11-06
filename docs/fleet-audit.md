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

## Simulation gaps

- [ ] 1. Beast heavy corvette burst attack: `HeavyCorvetteSpecialTarget` 0x5af8a0, `doBurstFire`
      0x5afb10, housekeep 0x5afcc0, order changed 0x5afd10, and the burst bullets exploding at the
      end of their flight (`bullet.rs`). The type has no module at all today.
- [ ] 2. Somtaaw destroyer missile regeneration: `StandardDestroyerHouseKeep` 0x5c2180.
- [ ] 3. Crystal harvesting at a depot: `salvDepotProcessCrystal` 0x4f4bb0 and the `consume*` code
      0x45b7b0..0x45c200 (clamp, beam, the crystal dissolving into RUs, `G_CrystalsHarvested`, the
      two fleet lines). Crystals take the generic convert today.
- [ ] 4. Beast depot process `salvDepotProcessBeast` 0x4f4a60 (the Beast infects what it salvages)
      and big gun depot process `salvDepotProcessBigGun` 0x4f4ec0.
- [ ] 5. The bombing run bombers make on non-ship targets (`GenericInterceptorAttack`).
- [ ] 6. `salvReleaseTarget` 0x4f5a00 and the rest of `salvHouseKeep` 0x4f7170: the "no depot"
      messages and letting go of a target for everyone at once. `salvageFlyToDistance` is not read.
- [ ] 7. Beast carrier does no damage in `fleet_coverage` (12 hidden gimbal guns, the Somtaaw
      carrier hits for 24000 in the same run): find out why. The gun-opening gate
      (`madOpenGunsShip`) is not modelled either.
- [ ] 8. Dreadnought: cannon reclamp after building (`cmDreadnoughtPostBuildReclamp` 0x451260) and
      `salvCheckDocked` on its clamped ships.
- [ ] 9. Link and unlink: the animations and the turn are skipped, and the merge happens on a timer
      (`processLinkToDo`, `processUnlinkToDo`).

## Player control gaps

- [ ] 10. Siege Cannon: the menu item shows but does nothing. `mrBigGunBeginTargeting` 0x560cf0,
      `mrBigGunCancel` 0x55f1c0, the targeting mode and the keys are missing; the simulation side
      (`mothership::cl_big_gun`) is there.
- [ ] 11. Beast mothership systems setting: `lmBeastSystemsSet` 0x49b210 (the launch manager's radio
      buttons, `mothership::set_damage_mode` has no caller).
- [ ] 12. Enter slipgate: `mrEnterNearestSlipgate` 0x560f50, `mrDockAtSlipgate` 0x5620f0.
- [ ] 13. Every command that still ends in "not available yet" in `Game::command`.
- [ ] 14. A Beast player end to end: build, research, launch manager, speech, parade. Beast is only
      ever an enemy in the campaign, so none of it has run with a real Beast player.

## Presentation gaps

- [ ] 15. Mesh animations started by the game: carrier and mothership doors, dock latch and unlatch
      (`dockLatchAnimForShipType`), hive open and close, Mimic link, sentinels, ramming frigates
      (`madLinkIn*` 0x4a0800..0x4a1310). Gun ports and harvesting already play.
- [ ] 16. Sounds, chatter and trail scaling of the hive and ramming frigates.
- [ ] 17. The Somtaaw carrier's second trail (`trail2*`).
- [ ] 18. Ships under the Naggarok's EMP (`rndShipUnderNaggarokEMP` 0x5704d0).
- [ ] 19. Remaining unread `.shp` keys: `scaleCap`, `badAI`, `fightmanAIP`.

Not gaps: `*_Save/_Load/_Fix/_PreFix` (the port saves differently), `MCV*` wrappers, and helper
functions the Rust inlines. The AI player for skirmish is multiplayer work and is left for then.
