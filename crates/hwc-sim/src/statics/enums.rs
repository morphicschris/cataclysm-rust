//! Enumerations the static data is keyed by, with the exact names and values of the original's
//! string tables (`StrToShipType` and friends). Name lookups ignore ASCII case, as the original's
//! do; an unknown name yields `None` where the original returns -1.

macro_rules! named_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident : $names:ident { $($variant:ident = $text:literal),* $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
        #[repr(u8)]
        $vis enum $name { $($variant),* }

        /// Names in value order, as spelled in the original's string table.
        $vis const $names: &[&str] = &[$($text),*];

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),*];
            pub const COUNT: usize = Self::ALL.len();

            /// Value as stored by the original (the enum ordinal).
            pub fn index(self) -> usize {
                self as usize
            }

            pub fn from_index(i: usize) -> Option<Self> {
                Self::ALL.get(i).copied()
            }

            pub fn name(self) -> &'static str {
                $names[self as usize]
            }

            /// Case-insensitive lookup, like the original's `StrTo...` functions.
            pub fn from_name(s: &str) -> Option<Self> {
                $names.iter().position(|n| n.eq_ignore_ascii_case(s)).and_then(Self::from_index)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.name())
            }
        }
    };
}

named_enum! {
    /// Every kind of ship, platform, component and ship-like derelict. Table at 0x87f438 in the
    /// executable, 145 entries.
    #[allow(non_camel_case_types)]
    pub enum ShipType: SHIP_TYPE_NAMES {
        sRecon = "sRecon", sAcolyte = "sAcolyte", sMimic = "sMimic", sLeech = "sLeech",
        sACV = "sACV", sMCV = "sMCV", sRammingFrigate = "sRammingFrigate",
        sMultiBeamFrigate = "sMultiBeamFrigate", sHiveFrigate = "sHiveFrigate",
        sSwarmer = "sSwarmer", sDreadnought = "sDreadnought",
        sDreadnoughtCannon = "sDreadnoughtCannon", sCarrier = "sCarrier",
        sCarrierSupport = "sCarrierSupport", sWorker = "sWorker", sRepairDroid = "sRepairDroid",
        sProcessor = "sProcessor", sSentinel = "sSentinel", sMothership = "sMothership",
        sMothershipSpecial = "sMothershipSpecial",
        sMothershipEngineering = "sMothershipEngineering",
        sMothershipArmour = "sMothershipArmour", sMothershipMicro = "sMothershipMicro",
        sMothershipWeapons = "sMothershipWeapons",
        sMothershipDockingBay = "sMothershipDockingBay", sDestroyer = "sDestroyer",
        sMothershipSupport = "sMothershipSupport", sMothershipCommand = "sMothershipCommand",
        sMothershipEnginesLower = "sMothershipEnginesLower",
        sMothershipEnginesUpper = "sMothershipEnginesUpper",
        sMothershipLowerSpine = "sMothershipLowerSpine",
        sMothershipBigGun = "sMothershipBigGun",
        sMothershipOreCanArmour = "sMothershipOreCanArmour",
        bRecon = "bRecon", bAcolyte = "bAcolyte", bInterceptor = "bInterceptor",
        bCloakedFighter = "bCloakedFighter", bAttackBomber = "bAttackBomber", bACV = "bACV",
        bMultiGunCorvette = "bMultiGunCorvette", bHeavyCorvette = "bHeavyCorvette",
        bMultiBeamFrigate = "bMultiBeamFrigate", bHiveFrigate = "bHiveFrigate",
        bSwarmer = "bSwarmer", bIonArrayFrigate = "bIonArrayFrigate",
        bDFGFrigate = "bDFGFrigate", bHeavyCruiser = "bHeavyCruiser", bCarrier = "bCarrier",
        bRammingFrigate = "bRammingFrigate", bWorker = "bWorker", bRepairDroid = "bRepairDroid",
        bProcessor = "bProcessor", bSentinel = "bSentinel", bMothership = "bMothership",
        bMothershipDockingBay = "bMothershipDockingBay",
        bMothershipEnginesLower = "bMothershipEnginesLower", bSectMoship = "bSectMoship",
        bMothershipSupport = "bMothershipSupport", bCruiseMissile = "bCruiseMissile",
        bMissileCorvette = "bMissileCorvette",
        kAdvanceSupportFrigate = "kAdvanceSupportFrigate", kAttackBomber = "kAttackBomber",
        kCarrier = "kCarrier", kCloakGenerator = "kCloakGenerator",
        kGravWellGenerator = "kGravWellGenerator", kHeavyCorvette = "kHeavyCorvette",
        kHeavyCruiser = "kHeavyCruiser", kHeavyDefender = "kHeavyDefender",
        kHeavyInterceptor = "kHeavyInterceptor", kIonCannonFrigate = "kIonCannonFrigate",
        kMultiGunCorvette = "kMultiGunCorvette", kProximitySensor = "kProximitySensor",
        kSalCapCorvette = "kSalCapCorvette", kStandardDestroyer = "kStandardDestroyer",
        tAdvanceSupportFrigate = "tAdvanceSupportFrigate", tAttackBomber = "tAttackBomber",
        tCarrier = "tCarrier", tCloakGenerator = "tCloakGenerator", tDFGFrigate = "tDFGFrigate",
        tGravWellGenerator = "tGravWellGenerator", tHeavyCruiser = "tHeavyCruiser",
        tHeavyDefender = "tHeavyDefender", tHeavyInterceptor = "tHeavyInterceptor",
        tIonCannonFrigate = "tIonCannonFrigate", tMinelayerCorvette = "tMinelayerCorvette",
        tMissileDestroyer = "tMissileDestroyer", tMultiGunCorvette = "tMultiGunCorvette",
        tProximitySensor = "tProximitySensor", tResourceCollector = "tResourceCollector",
        tSalCapCorvette = "tSalCapCorvette", tStandardDestroyer = "tStandardDestroyer",
        tStandardFrigate = "tStandardFrigate",
        p1Outpost = "p1Outpost", p1Fighter = "p1Fighter", p1IonArrayFrigate = "p1IonArrayFrigate",
        p1MissileCorvette = "p1MissileCorvette", p1Mothership = "p1Mothership",
        p1StandardCorvette = "p1StandardCorvette",
        trFloatingCity = "trFloatingCity", trCargoBarge = "trCargoBarge",
        trMiningBase = "trMiningBase", trResearchStation = "trResearchStation",
        trJunkYardDawg = "trJunkYardDawg", trJunkYardHQ = "trJunkYardHQ",
        trGhostship = "trGhostship", trJunk_LGun = "trJunk_LGun", trJunk_SGun = "trJunk_SGun",
        trResearchStationBridge = "trResearchStationBridge",
        trResearchStationTower = "trResearchStationTower",
        dRKO = "dRKO", dBigGun = "dBigGun", dBigGunUnit = "dBigGunUnit",
        dSlipgateSmall = "dSlipgateSmall", dSlipgateMedium = "dSlipgateMedium",
        dSlipgateLarge = "dSlipgateLarge", dCrate = "dCrate", dJunk1PartA = "dJunk1PartA",
        dJunk1Shell = "dJunk1Shell", dJunk1Strut = "dJunk1Strut",
        dJunk2Shipwreck = "dJunk2Shipwreck", dJunk3Boiler = "dJunk3Boiler",
        dJunk3BoilerCasing = "dJunk3BoilerCasing", dMessagePod = "dMessagePod", dBuoy = "dBuoy",
        dFirstPlanet = "dFirstPlanet", dDeadPlanet = "dDeadPlanet", dCrystal0 = "dCrystal0",
        dCrystal1 = "dCrystal1", dBeastDockingBay = "dBeastDockingBay", dNISBuoy = "dNISBuoy",
        dBeastMothership = "dBeastMothership", dNomadMoon = "dNomadMoon",
        dNomadFieldEmitter = "dNomadFieldEmitter", dAcolyte = "dAcolyte",
        dPrisonShip = "dPrisonShip", dPrisonShipInfected = "dPrisonShipInfected",
        dSlipgateNode = "dSlipgateNode", dKushanCarrier = "dKushanCarrier",
        dIonArrayFrigate = "dIonArrayFrigate", dIonCannonFrigate = "dIonCannonFrigate",
        dEcclesiam = "dEcclesiam", dEcclesiamInfected = "dEcclesiamInfected",
        nisLightSource1 = "nisLightSource1", nisLightSource2 = "nisLightSource2",
        nisLensflareSource1 = "nisLensflareSource1",
    }
}

named_enum! {
    /// Races. The name doubles as the data directory (`RACE_SECT\`). Table at 0x87f8c8.
    pub enum ShipRace: SHIP_RACE_NAMES {
        Somtaaw = "RACE_SECT",
        Beast = "RACE_BEAST",
        Kushan = "RACE_KUSHAN",
        Taiidan = "RACE_TAIDAN",
        Turanic = "RACE_P1",
        Traders = "RACE_TRADERS",
        Derelict = "RACE_DERELICT",
    }
}

/// First and last ship type of each race, in race order (tables at 0x87f3f0 and 0x87f40c).
const RACE_TYPE_RANGES: [(u8, u8); 7] = [(0, 32), (33, 59), (60, 73), (74, 91), (92, 97), (98, 108), (109, 144)];

impl ShipRace {
    /// The ship types belonging to this race, in type order.
    pub fn ship_types(self) -> impl Iterator<Item = ShipType> {
        let (first, last) = RACE_TYPE_RANGES[self.index()];
        (first..=last).filter_map(|i| ShipType::from_index(i.into()))
    }
}

impl ShipType {
    /// The race whose data directory holds this ship.
    pub fn race(self) -> ShipRace {
        let i = self as u8;
        let r = RACE_TYPE_RANGES
            .iter()
            .position(|&(a, b)| (a..=b).contains(&i))
            .unwrap_or(ShipRace::Derelict.index());
        ShipRace::ALL[r]
    }

    /// The original's "strike craft" set (`formationPaddingModifier` gets the strike-craft
    /// default). Not the same as "fighters and corvettes": several corvettes are missing.
    pub fn is_strike_craft(self) -> bool {
        use ShipType::*;
        matches!(
            self,
            sRecon | sAcolyte | sMimic | sLeech | sACV | sMCV | bRecon | bAcolyte | bInterceptor
                | bCloakedFighter | bAttackBomber | bACV | kAttackBomber | kHeavyCorvette
                | kHeavyDefender | kHeavyInterceptor | kMultiGunCorvette | kSalCapCorvette
                | tAttackBomber | tHeavyDefender | tHeavyInterceptor | tMinelayerCorvette
                | tMultiGunCorvette | tSalCapCorvette | p1Fighter | p1MissileCorvette
                | p1StandardCorvette
        )
    }
}

named_enum! {
    /// Ship classes. Table at 0x87f988.
    pub enum ShipClass: SHIP_CLASS_NAMES {
        Mothership = "CLASS_Mothership",
        HeavyCruiser = "CLASS_HeavyCruiser",
        Carrier = "CLASS_Carrier",
        Destroyer = "CLASS_Destroyer",
        Frigate = "CLASS_Frigate",
        Corvette = "CLASS_Corvette",
        Fighter = "CLASS_Fighter",
        Resource = "CLASS_Resource",
        NonCombat = "CLASS_NonCombat",
        MonsterShip = "CLASS_MonsterShip",
        Component = "CLASS_Component",
        Micro = "CLASS_Micro",
    }
}

named_enum! {
    /// Gun mounting types. Table at 0x87fb10; `GUN_HiddenGimble` and `GUN_VisibleGimble` are
    /// aliases resolved by [`GunType::from_script_name`].
    pub enum GunType: GUN_TYPE_NAMES {
        Fixed = "GUN_Fixed",
        Gimble = "GUN_Gimble",
        NewGimble = "GUN_NewGimble",
        MissileLauncher = "GUN_MissileLauncher",
        MineLauncher = "GUN_MineLauncher",
    }
}

impl GunType {
    /// Lookup including the Cataclysm aliases (`GUN_HiddenGimble` is a gimbal gun whose barrel
    /// is not modelled, `GUN_VisibleGimble` one that is).
    pub fn from_script_name(s: &str) -> Option<Self> {
        if s.eq_ignore_ascii_case("GUN_HiddenGimble") {
            Some(GunType::Gimble)
        } else if s.eq_ignore_ascii_case("GUN_VisibleGimble") {
            Some(GunType::NewGimble)
        } else {
            Self::from_name(s)
        }
    }
}

named_enum! {
    /// Gun sizes (Cataclysm). Table at 0x87fd88.
    pub enum GunSize: GUN_SIZE_NAMES {
        Small = "GUNSIZE_SMALL",
        Medium = "GUNSIZE_MEDIUM",
        Large = "GUNSIZE_LARGE",
        VeryLarge = "GUNSIZE_VERYLARGE",
    }
}

named_enum! {
    /// Weapon technology levels a gun can be upgraded between (Cataclysm). Table at 0x87fdb0.
    pub enum WeaponType: WEAPON_TYPE_NAMES {
        Projectile = "WEAPON_PROJECTILE",
        Energy = "WEAPON_ENERGY",
        Ion = "WEAPON_ION",
        PlasmaBomb = "WEAPON_PLASMABOMB",
        Laser = "WEAPON_LASER",
        Missile = "WEAPON_MISSILE",
        Mine = "WEAPON_MINE",
        BigGun = "WEAPON_BIGGUN",
        BeastInfect = "WEAPON_BEASTINFECT",
    }
}

named_enum! {
    /// Docking point types. Table at 0x87fb50.
    pub enum DockPointType: DOCK_POINT_TYPE_NAMES {
        SmallIn = "DPT_Small_In",
        SmallOut = "DPT_Small_Out",
        SmallInEngineering = "DPT_Small_In_Engineering",
        SmallOutEngineering = "DPT_Small_Out_Engineering",
        FighterLatch = "DPT_Fighter_Latch",
        CorvetteLatch = "DPT_Corvette_Latch",
        MedIn = "DPT_Med_In",
        MedOut = "DPT_Med_Out",
        MedInEngineering = "DPT_Med_In_Engineering",
        MedOutEngineering = "DPT_Med_Out_Engineering",
        LargeIn = "DPT_Large_In",
        LargeOut = "DPT_Large_Out",
        Leech = "DPT_Leech",
        LeechRuTransfer = "DPT_Leech_RU_Transfer",
        WorkerRuTransfer = "DPT_Worker_RU_Transfer",
        Tow = "DPT_Tow",
        Link = "DPT_Link",
        Repairbot = "DPT_Repairbot",
        DestroyerEb = "DPT_Destroyer_EB",
        ModuleEb = "DPT_Module_EB",
        SupportEb = "DPT_Support_EB",
        SwarmerIn = "DPT_Swarmer_In",
        SwarmerOut = "DPT_Swarmer_Out",
        CrystalProcess = "DPT_Crystal_Process",
        CrystalEffect = "DPT_Crystal_Effect",
        ForceFieldOrigin = "DPT_ForceFieldOrigin",
        ResourcerRuTransfer = "DPT_Resourcer_RU_Transfer",
        Salvage = "DPT_Salvage",
        SalvageLatch = "DPT_SalvageLatch",
        SalvageProcess = "DPT_SalvageProcess",
        RkoWeaponSource = "DPT_RKOWeaponSource",
        AutoRepair = "DPT_AutoRepair",
        CleesanBoarding = "DPT_CleesanBoarding",
        MothershipIn = "DPT_Mothership_In",
        MothershipOut = "DPT_Mothership_Out",
    }
}

named_enum! {
    /// Navigation light behaviours. Table at 0x87fc90.
    pub enum NavLightType: NAV_LIGHT_TYPE_NAMES {
        Default = "NAVLIGHT_Default",
        FadeOnOff = "NAVLIGHT_FadeOnOff",
    }
}

named_enum! {
    /// Asteroid kinds (resource files `Resources\Asteroids\AsteroidN.shp`).
    pub enum AsteroidType: ASTEROID_TYPE_NAMES {
        Asteroid0 = "Asteroid0", Asteroid1 = "Asteroid1", Asteroid2 = "Asteroid2",
        Asteroid3 = "Asteroid3", Asteroid4 = "Asteroid4",
    }
}

named_enum! {
    /// Dust cloud kinds.
    pub enum DustCloudType: DUST_CLOUD_TYPE_NAMES {
        DustCloud0 = "DustCloud0", DustCloud1 = "DustCloud1",
        DustCloud2 = "DustCloud2", DustCloud3 = "DustCloud3",
    }
}

named_enum! {
    /// Nebula kinds (Cataclysm has one).
    pub enum NebulaType: NEBULA_TYPE_NAMES {
        Nebula0 = "Nebula0",
    }
}

named_enum! {
    /// Old-style derelicts (`Derelicts\`). Most of Cataclysm's derelicts are ship types of
    /// `RACE_DERELICT` instead; only these eight remain. `HyperspaceGate` has no file and is
    /// built in code.
    pub enum DerelictType: DERELICT_TYPE_NAMES {
        Crate = "Crate", Homeworld = "Homeworld", PlanetOfOrigin = "PlanetOfOrigin",
        PlanetOfOriginScarred = "PlanetOfOrigin_scarred", Galaxy = "Galaxy",
        LavaBall = "LavaBall", MattWorld = "MattWorld", HyperspaceGate = "HyperspaceGate",
    }
}

named_enum! {
    /// Research technologies (Cataclysm has 41). Table at 0x8860e0.
    pub enum Technology: TECHNOLOGY_NAMES {
        Cloaking = "TECH_CLOAKING",
        TargetingSystems = "TECH_TARGETING_SYSTEMS",
        PlasmaWeapons = "TECH_PLASMA_WEAPONS",
        MediumGuns = "TECH_MEDIUM_GUNS",
        DefenseField = "TECH_DEFENSE_FIELD",
        HeavyGuns = "TECH_HEAVY_GUNS",
        Repair = "TECH_REPAIR",
        Salvage = "TECH_SALVAGE",
        DriveFighter = "TECH_DRIVE_FIGHTER",
        AdriveFighter = "TECH_ADRIVE_FIGHTER",
        DriveFrigate = "TECH_DRIVE_FRIGATE",
        AdriveFrigate = "TECH_ADRIVE_FRIGATE",
        DriveDestroyer = "TECH_DRIVE_DESTROYER",
        AdriveDestroyer = "TECH_ADRIVE_DESTROYER",
        Armour2 = "TECH_ARMOUR2",
        Armour3 = "TECH_ARMOUR3",
        SupportSystems = "TECH_SUPPORT_SYSTEMS",
        MicroShip = "TECH_MICRO_SHIP",
        Energy = "TECH_ENERGY",
        Ion = "TECH_ION",
        Missile = "TECH_MISSILE",
        Repulse = "TECH_REPULSE",
        Emp = "TECH_EMP",
        ExplosiveCharge = "TECH_EXPLOSIVE_CHARGE",
        Link = "TECH_LINK",
        Mimic = "TECH_MIMIC",
        Afterburner = "TECH_AFTERBURNER",
        BigGun = "TECH_BIG_GUN",
        AdvancedSensors = "TECH_ADVANCED_SENSORS",
        ForceField1 = "TECH_FORCE_FIELD1",
        ForceField2 = "TECH_FORCE_FIELD2",
        ForceField3 = "TECH_FORCE_FIELD3",
        Infect = "TECH_INFECT",
        Repairbots = "TECH_REPAIRBOTS",
        CrystalHarvest = "TECH_CRYSTAL_HARVEST",
        ResearchCleesanAttack = "TECH_RESEARCH_CLEESAN_ATTACK",
        BeastVaccine = "TECH_BEAST_VACCINE",
        PhasedTelemetry = "TECH_PHASED_TELEMETRY",
        InertialessDrive = "TECH_INERTIALESS_DRIVE",
        Biowarhead = "TECH_BIOWARHEAD",
        DriveCorvette = "TECH_DRIVE_CORVETTE",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables() {
        assert_eq!(ShipType::COUNT, 145);
        assert_eq!(ShipType::nisLensflareSource1.index(), 144);
        assert_eq!(ShipType::from_name("SACOLYTE"), Some(ShipType::sAcolyte));
        assert_eq!(ShipType::bWorker.index(), 49);
        assert_eq!(ShipType::trMiningBase.index(), 100);
        assert_eq!(ShipType::kSalCapCorvette.race(), ShipRace::Kushan);
        assert_eq!(ShipRace::Somtaaw.ship_types().count(), 33);
        assert_eq!(ShipRace::Derelict.ship_types().count(), 36);
        let total: usize = ShipRace::ALL.iter().map(|r| r.ship_types().count()).sum();
        assert_eq!(total, 145);
        assert_eq!(GunType::from_script_name("GUN_HiddenGimble"), Some(GunType::Gimble));
        assert_eq!(GunSize::from_name("GUNSIZE_Medium"), Some(GunSize::Medium));
        assert_eq!(Technology::COUNT, 41);
        assert_eq!(DockPointType::COUNT, 35);
    }
}
