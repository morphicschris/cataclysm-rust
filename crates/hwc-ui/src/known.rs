//! Every callback name the shipped `.fib` files bind (atoms with the function flag, list
//! windows and text entries) and every user-region draw callback name, with the files that
//! use them. Names without an implementation are registered as logging stubs.

/// Function callbacks: (name, files).
pub const FUNCTIONS: &[(&str, &[&str])] = &[
    ("AbortHyperspace", &["hyperspace_roll_call.fib"]),
    (
        "CM_BuildShips",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    (
        "CM_CancelJobs",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    (
        "CM_Close",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    (
        "CM_ExtendedInfo",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    (
        "CM_Launch",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    (
        "CM_PauseJobs",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    (
        "CM_Research",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    (
        "CM_Scroller",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    ("CON_TextDraw", &["console.fib"]),
    (
        "CP_BaseColor",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "multiplayer_game.fib",
        ],
    ),
    (
        "CP_Default",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "multiplayer_game.fib",
        ],
    ),
    (
        "CP_Previous0",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "multiplayer_game.fib",
        ],
    ),
    (
        "CP_Previous1",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "multiplayer_game.fib",
        ],
    ),
    (
        "CP_Previous2",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "multiplayer_game.fib",
        ],
    ),
    (
        "CP_Previous3",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "multiplayer_game.fib",
        ],
    ),
    (
        "CP_Previous4",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "multiplayer_game.fib",
        ],
    ),
    (
        "CP_RaceOne",
        &["front_end_demo.fib", "multiplayer_game.fib"],
    ),
    (
        "CP_RaceTwo",
        &["front_end_demo.fib", "multiplayer_game.fib"],
    ),
    (
        "CP_StripeColor",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "multiplayer_game.fib",
        ],
    ),
    ("CSM_Agressive", &["csm-all.fib"]),
    ("CSM_BroadFormation", &["csm-all.fib"]),
    ("CSM_Build", &["csm-all.fib", "taskbar.fib"]),
    ("CSM_Cancel", &["csm-all.fib"]),
    ("CSM_ClawFormation", &["csm-all.fib"]),
    ("CSM_DeltaFormation", &["csm-all.fib"]),
    ("CSM_Dock", &["csm-all.fib"]),
    ("CSM_Evasive", &["csm-all.fib"]),
    ("CSM_Harvest", &["csm-all.fib"]),
    ("CSM_Hyperspace", &["taskbar.fib"]),
    ("CSM_Launch", &["csm-all.fib", "taskbar.fib"]),
    ("CSM_Move", &["csm-all.fib"]),
    ("CSM_Neutral", &["csm-all.fib"]),
    ("CSM_PicketFormation", &["csm-all.fib"]),
    ("CSM_Research", &["csm-all.fib", "taskbar.fib"]),
    ("CSM_Retire", &["csm-all.fib"]),
    ("CSM_Scuttle", &["csm-all.fib"]),
    ("CSM_SphereFormation", &["csm-all.fib"]),
    ("CSM_WallFormation", &["csm-all.fib"]),
    ("CSM_XFormation", &["csm-all.fib"]),
    (
        "CS_Back",
        &["multiplayer_game.fib", "multiplayer_lan_game.fib"],
    ),
    (
        "CS_Done",
        &["multiplayer_game.fib", "multiplayer_lan_game.fib"],
    ),
    ("CS_ScenarioBitmap", &["multiplayer_game.fib"]),
    (
        "CS_ScenarioWindowInit",
        &["multiplayer_game.fib", "multiplayer_lan_game.fib"],
    ),
    ("Disappear", &["csm-all.fib"]),
    ("EmergencyHyperspace", &["hyperspace_roll_call.fib"]),
    (
        "FE_GameWindowInit",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
            "multiplayer_game.fib",
        ],
    ),
    (
        "FE_HideMenuOrGotoQuitGame",
        &["front_end.fib", "front_end_demo.fib"],
    ),
    ("FE_LessonGameWindowInit", &["in_game_esc_menu.fib"]),
    (
        "FE_Multiplayer_Game",
        &["front_end.fib", "front_end_demo.fib"],
    ),
    (
        "FE_RecordedGameWindowInit",
        &["in_game_esc_menu.fib", "multiplayer_game.fib"],
    ),
    ("FE_SHOWCREDITS", &["front_end.fib"]),
    (
        "FE_TutorialGameWindowInit",
        &["front_end.fib", "front_end_demo.fib"],
    ),
    ("GoNowHyperspace", &["hyperspace_roll_call.fib"]),
    ("HR_AbortLoadingNo", &["horse_race.fib"]),
    ("HR_AbortLoadingYes", &["horse_race.fib"]),
    ("HR_ChatTextEntry", &["horse_race.fib"]),
    ("IG_Cancel", &["in_game_esc_menu.fib"]),
    (
        "IG_DeleteGame",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
            "multiplayer_game.fib",
        ],
    ),
    ("IG_LoadGame", &["in_game_esc_menu.fib"]),
    ("IG_SaveGame", &["in_game_esc_menu.fib"]),
    ("IG_SaveLessonGame", &["in_game_esc_menu.fib"]),
    ("IG_SaveRecordedGame", &["in_game_esc_menu.fib"]),
    ("IG_StartEscapeMenu", &["in_game_esc_menu.fib"]),
    ("IG_StopRecording", &["in_game_esc_menu.fib"]),
    ("IG_TextEntryWindowInit", &["in_game_esc_menu.fib"]),
    ("InGameChatEntry", &["game_chat.fib"]),
    ("LG_BackFromPassword", &["multiplayer_lan_game.fib"]),
    ("LG_BackToConnection", &["multiplayer_lan_game.fib"]),
    ("LG_BackToLogin", &["multiplayer_lan_game.fib"]),
    ("LG_ChangeColors", &["multiplayer_lan_game.fib"]),
    ("LG_ChatHistoryWindow", &["multiplayer_lan_game.fib"]),
    ("LG_ChatTextEntry", &["multiplayer_lan_game.fib"]),
    ("LG_CreateGame", &["multiplayer_lan_game.fib"]),
    ("LG_DontQuitWON", &["multiplayer_lan_game.fib"]),
    ("LG_GameChatTextEntry", &["multiplayer_lan_game.fib"]),
    ("LG_GameChatWindow", &["multiplayer_lan_game.fib"]),
    ("LG_GamePasswordEntry", &["multiplayer_lan_game.fib"]),
    ("LG_GameUserName", &["multiplayer_lan_game.fib"]),
    ("LG_GoPassword", &["multiplayer_lan_game.fib"]),
    ("LG_JoinGame", &["multiplayer_lan_game.fib"]),
    ("LG_LaunchLAN", &["multiplayer_lan_game.fib"]),
    ("LG_LeaveGame", &["multiplayer_lan_game.fib"]),
    ("LG_ListOfGames", &["multiplayer_lan_game.fib"]),
    ("LG_MessageOk", &["multiplayer_lan_game.fib"]),
    ("LG_NameEntry", &["multiplayer_lan_game.fib"]),
    ("LG_SeeDetails", &["multiplayer_lan_game.fib"]),
    ("LG_SetupGame", &["multiplayer_lan_game.fib"]),
    ("LG_StartGame", &["multiplayer_lan_game.fib"]),
    ("LG_UserNameWindow", &["multiplayer_lan_game.fib"]),
    ("LG_ViewDetails", &["multiplayer_lan_game.fib"]),
    ("LG_YesQuitWON", &["multiplayer_lan_game.fib"]),
    (
        "LM_AutoLaunchC1",
        &["beast_launch_manager.fib", "launch_manager.fib"],
    ),
    (
        "LM_AutoLaunchC2",
        &["beast_launch_manager.fib", "launch_manager.fib"],
    ),
    (
        "LM_AutoLaunchM",
        &["beast_launch_manager.fib", "launch_manager.fib"],
    ),
    ("LM_BeastSystemsSet", &["beast_launch_manager.fib"]),
    (
        "LM_Close",
        &["beast_launch_manager.fib", "launch_manager.fib"],
    ),
    (
        "LM_Construction",
        &["beast_launch_manager.fib", "launch_manager.fib"],
    ),
    (
        "LM_Launch",
        &["beast_launch_manager.fib", "launch_manager.fib"],
    ),
    (
        "LM_LaunchAll",
        &["beast_launch_manager.fib", "launch_manager.fib"],
    ),
    (
        "LM_LaunchOne",
        &["beast_launch_manager.fib", "launch_manager.fib"],
    ),
    (
        "LM_Research",
        &["beast_launch_manager.fib", "launch_manager.fib"],
    ),
    (
        "LM_ShipsToLaunch",
        &["beast_launch_manager.fib", "launch_manager.fib"],
    ),
    (
        "LoadMultiSinglePlayerGameFromMain",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "multiplayer_game.fib",
        ],
    ),
    ("LoadRecordedGame", &["multiplayer_game.fib"]),
    ("MG_AbortDownloadPatch", &["multiplayer_game.fib"]),
    ("MG_AdvancedOptions", &["multiplayer_game.fib"]),
    ("MG_AgeRange", &["multiplayer_game.fib"]),
    ("MG_AlliedVictory", &["multiplayer_game.fib"]),
    ("MG_AllowAllies", &["multiplayer_game.fib"]),
    ("MG_BackFromOptions", &["multiplayer_game.fib"]),
    ("MG_BackFromPassword", &["multiplayer_game.fib"]),
    ("MG_BackFromPlayerOptions", &["multiplayer_game.fib"]),
    ("MG_BackFromRoomPassword", &["multiplayer_game.fib"]),
    ("MG_BackToAvailableChannels", &["multiplayer_game.fib"]),
    ("MG_BackToChannelChat", &["multiplayer_game.fib"]),
    ("MG_BackToConnection", &["multiplayer_game.fib"]),
    ("MG_BackToLogin", &["multiplayer_game.fib"]),
    ("MG_BasicOptions", &["multiplayer_game.fib"]),
    ("MG_BountiesEnabled", &["multiplayer_game.fib"]),
    ("MG_BountySize", &["multiplayer_game.fib"]),
    ("MG_CancelChooseServer", &["choose_server.fib"]),
    (
        "MG_ChangeColors",
        &["multiplayer_game.fib", "multiplayer_lan_game.fib"],
    ),
    ("MG_ChangePassword", &["multiplayer_game.fib"]),
    ("MG_ChangePasswordBack", &["multiplayer_game.fib"]),
    ("MG_ChangePasswordNow", &["multiplayer_game.fib"]),
    ("MG_ChannelConfirmEntry", &["multiplayer_game.fib"]),
    ("MG_ChannelDescriptionEntry", &["multiplayer_game.fib"]),
    ("MG_ChannelNameEntry", &["multiplayer_game.fib"]),
    ("MG_ChannelPasswordEntry", &["multiplayer_game.fib"]),
    ("MG_ChannelProtected", &["multiplayer_game.fib"]),
    ("MG_ChatHistoryWindow", &["multiplayer_game.fib"]),
    ("MG_ChatTextEntry", &["multiplayer_game.fib"]),
    ("MG_ChooseScenario", &["multiplayer_game.fib"]),
    ("MG_ConfirmPasswordChangeEntry", &["multiplayer_game.fib"]),
    ("MG_ConnectingCancel", &["multiplayer_game.fib"]),
    ("MG_ConnectingStatus", &["multiplayer_game.fib"]),
    ("MG_ConnectionBack", &["multiplayer_game.fib"]),
    ("MG_ContestServer", &["multiplayer_game.fib"]),
    ("MG_CountryListWindowInit", &["multiplayer_game.fib"]),
    ("MG_CratesEnabled", &["multiplayer_game.fib"]),
    ("MG_CreateChannel", &["multiplayer_game.fib"]),
    ("MG_CreateChannelNow", &["multiplayer_game.fib"]),
    ("MG_CreateGame", &["multiplayer_game.fib"]),
    ("MG_CreateGameNow", &["multiplayer_game.fib"]),
    ("MG_CreateNewAccountNow", &["multiplayer_game.fib"]),
    ("MG_DontQuitWON", &["multiplayer_game.fib"]),
    ("MG_EMail", &["multiplayer_game.fib"]),
    ("MG_FirewallButton", &["multiplayer_game.fib"]),
    ("MG_GameChatTextEntry", &["multiplayer_game.fib"]),
    ("MG_GameChatWindow", &["multiplayer_game.fib"]),
    ("MG_GameNameTextEntry", &["multiplayer_game.fib"]),
    ("MG_GamePassword", &["multiplayer_game.fib"]),
    ("MG_GamePasswordConfirm", &["multiplayer_game.fib"]),
    ("MG_GamePasswordEntry", &["multiplayer_game.fib"]),
    ("MG_GameType", &["multiplayer_game.fib"]),
    ("MG_GameUserName", &["multiplayer_game.fib"]),
    ("MG_Gender", &["multiplayer_game.fib"]),
    ("MG_GoChooseServer", &["choose_server.fib"]),
    ("MG_GoPassword", &["multiplayer_game.fib"]),
    ("MG_GoRoomPassword", &["multiplayer_game.fib"]),
    ("MG_HarvestingEnabled", &["multiplayer_game.fib"]),
    ("MG_Hyperspace", &["multiplayer_game.fib"]),
    ("MG_InternetWON", &["multiplayer_game.fib"]),
    ("MG_JoinChannel", &["multiplayer_game.fib"]),
    ("MG_JoinGame", &["multiplayer_game.fib"]),
    ("MG_LANIPX", &["multiplayer_game.fib"]),
    ("MG_LastMotherShip", &["multiplayer_game.fib"]),
    ("MG_LaunchLAN", &["multiplayer_game.fib"]),
    ("MG_LaunchWON", &["multiplayer_game.fib"]),
    ("MG_LeaveGame", &["multiplayer_game.fib"]),
    ("MG_ListOfChannels", &["multiplayer_game.fib"]),
    ("MG_ListOfGames", &["multiplayer_game.fib"]),
    ("MG_ListOfGamesBack", &["multiplayer_game.fib"]),
    ("MG_ListOfServers", &["choose_server.fib"]),
    ("MG_MessageOk", &["multiplayer_game.fib"]),
    ("MG_NameEntry", &["multiplayer_game.fib"]),
    ("MG_NewAccount", &["multiplayer_game.fib"]),
    ("MG_NewAccountBack", &["multiplayer_game.fib"]),
    ("MG_NewAccountConfirm", &["multiplayer_game.fib"]),
    ("MG_NewAccountPassword", &["multiplayer_game.fib"]),
    ("MG_NewPasswordChangeEntry", &["multiplayer_game.fib"]),
    ("MG_NoDownloadPatch", &["multiplayer_game.fib"]),
    ("MG_OldPasswordChangeEntry", &["multiplayer_game.fib"]),
    ("MG_PasswordEntry", &["multiplayer_game.fib"]),
    ("MG_PasswordProtected", &["multiplayer_game.fib"]),
    ("MG_PingAllGames", &["multiplayer_game.fib"]),
    ("MG_ReallyCreateNewAccountNow", &["multiplayer_game.fib"]),
    ("MG_RegistrationBack", &["multiplayer_game.fib"]),
    ("MG_ResearchEnabled", &["multiplayer_game.fib"]),
    ("MG_ResourceInjectionAmount", &["multiplayer_game.fib"]),
    ("MG_ResourceInjectionInterval", &["multiplayer_game.fib"]),
    ("MG_ResourceInjections", &["multiplayer_game.fib"]),
    ("MG_ResourceLumpSum", &["multiplayer_game.fib"]),
    ("MG_ResourceLumpSumAmount", &["multiplayer_game.fib"]),
    ("MG_ResourceLumpSumInterval", &["multiplayer_game.fib"]),
    ("MG_ResourceOptions", &["multiplayer_game.fib"]),
    ("MG_RoomPasswordEntry", &["multiplayer_game.fib"]),
    ("MG_SendInformation", &["multiplayer_game.fib"]),
    ("MG_SetColorsNow", &["multiplayer_game.fib"]),
    ("MG_SetupGame", &["multiplayer_game.fib"]),
    ("MG_SierraCom", &["multiplayer_game.fib"]),
    ("MG_Skirmish", &["multiplayer_game.fib"]),
    ("MG_StartGame", &["multiplayer_game.fib"]),
    ("MG_StartingResources", &["multiplayer_game.fib"]),
    ("MG_UserNameWindow", &["multiplayer_game.fib"]),
    ("MG_ViewChannels", &["multiplayer_game.fib"]),
    ("MG_ViewDetails", &["multiplayer_game.fib"]),
    ("MG_ViewGames", &["multiplayer_game.fib"]),
    ("MG_YesDownloadPatch", &["multiplayer_game.fib"]),
    ("MG_YesQuitWON", &["multiplayer_game.fib"]),
    ("MG_Zip", &["multiplayer_game.fib"]),
    (
        "OP_Advanced_To_Audio_Accept",
        &["front_end.fib", "front_end_demo.fib"],
    ),
    (
        "OP_Advanced_To_Audio_Cancel",
        &["front_end.fib", "front_end_demo.fib"],
    ),
    (
        "OP_AreaExplored",
        &["front_end.fib", "in_game_esc_menu.fib"],
    ),
    (
        "OP_AutoChannels",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_AutoFocusOnWaypoints",
        &["front_end.fib", "in_game_esc_menu.fib"],
    ),
    ("OP_Background", &["front_end.fib", "in_game_esc_menu.fib"]),
    (
        "OP_BattleChatter",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_Brightness",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    ("OP_CPU_Attacks", &["multiplayer_game.fib"]),
    ("OP_CPU_Difficulty", &["multiplayer_game.fib"]),
    (
        "OP_CapitalShipOverlay",
        &["front_end.fib", "in_game_esc_menu.fib"],
    ),
    (
        "OP_CataclysmDefault",
        &["front_end.fib", "in_game_esc_menu.fib"],
    ),
    ("OP_CataclysmFramerate", &["front_end.fib"]),
    ("OP_Compass", &["front_end.fib", "in_game_esc_menu.fib"]),
    ("OP_Countdown_No", &["front_end.fib", "front_end_demo.fib"]),
    ("OP_Countdown_Yes", &["front_end.fib", "front_end_demo.fib"]),
    (
        "OP_DCTQuality",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_Detail_Threshold",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    ("OP_Dust", &["front_end.fib", "in_game_esc_menu.fib"]),
    (
        "OP_Effects",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_Effects_To_InGameVideo_Accept",
        &["in_game_esc_menu.fib"],
    ),
    (
        "OP_Effects_To_InGameVideo_Cancel",
        &["in_game_esc_menu.fib"],
    ),
    (
        "OP_Effects_To_Video_Accept",
        &["front_end.fib", "front_end_demo.fib"],
    ),
    (
        "OP_Effects_To_Video_Cancel",
        &["front_end.fib", "front_end_demo.fib"],
    ),
    (
        "OP_Equalizer1",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_Equalizer2",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_Equalizer3",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_Equalizer4",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_Equalizer5",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_Equalizer6",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_Equalizer7",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_Equalizer8",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_Equalizer_To_Audio_Accept",
        &["front_end.fib", "front_end_demo.fib"],
    ),
    (
        "OP_Equalizer_To_Audio_Cancel",
        &["front_end.fib", "front_end_demo.fib"],
    ),
    (
        "OP_ExperienceMessages",
        &["front_end.fib", "in_game_esc_menu.fib"],
    ),
    ("OP_HWTaskbar", &["front_end.fib", "in_game_esc_menu.fib"]),
    ("OP_Hear_Voice_1", &["front_end_demo.fib"]),
    ("OP_Hear_Voice_2", &["front_end_demo.fib"]),
    ("OP_Hear_Voice_3", &["front_end_demo.fib"]),
    (
        "OP_HomeworldCSA",
        &["front_end.fib", "in_game_esc_menu.fib"],
    ),
    (
        "OP_HomeworldClassic",
        &["front_end.fib", "in_game_esc_menu.fib"],
    ),
    ("OP_InGameOptions_Accept", &["in_game_esc_menu.fib"]),
    ("OP_InGameOptions_Cancel", &["in_game_esc_menu.fib"]),
    ("OP_InGameOptions_Start", &["in_game_esc_menu.fib"]),
    ("OP_Info_Overlay", &["front_end_demo.fib"]),
    (
        "OP_KeyBindingsList",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_KeyBindingsPoolList",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_KeyResetToDefault",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    ("OP_LODDetail", &["front_end.fib", "in_game_esc_menu.fib"]),
    (
        "OP_Mouse_Sensitivity",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_Music_Volume",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_NoLOD",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_NumEffects",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_Num_Channels",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_ObjectivePointers",
        &["front_end.fib", "in_game_esc_menu.fib"],
    ),
    (
        "OP_Options_Accept",
        &["front_end.fib", "front_end_demo.fib"],
    ),
    (
        "OP_Options_Cancel",
        &["front_end.fib", "front_end_demo.fib"],
    ),
    ("OP_Options_Start", &["front_end.fib", "front_end_demo.fib"]),
    ("OP_Render", &["front_end.fib", "front_end_demo.fib"]),
    ("OP_Resolution", &["front_end.fib", "front_end_demo.fib"]),
    ("OP_Resources", &["front_end.fib", "in_game_esc_menu.fib"]),
    (
        "OP_RightClickCSA",
        &["front_end.fib", "in_game_esc_menu.fib"],
    ),
    (
        "OP_SFX_Volume",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_SensorsSphere",
        &["front_end.fib", "in_game_esc_menu.fib"],
    ),
    (
        "OP_SensorsWireframe",
        &["front_end.fib", "in_game_esc_menu.fib"],
    ),
    (
        "OP_SlipgatePaths",
        &["front_end.fib", "in_game_esc_menu.fib"],
    ),
    ("OP_SmallFonts", &["front_end.fib", "in_game_esc_menu.fib"]),
    ("OP_SoundFX_Test", &["front_end.fib"]),
    (
        "OP_Speaker",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    ("OP_Speech_Test", &["front_end.fib"]),
    (
        "OP_Speech_Volume",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_TaskbarBackdrop",
        &["front_end.fib", "in_game_esc_menu.fib"],
    ),
    (
        "OP_TaskbarButtons",
        &["front_end.fib", "in_game_esc_menu.fib"],
    ),
    ("OP_Texture_Detail", &["front_end.fib"]),
    ("OP_TroubleShoot", &["front_end.fib", "front_end_demo.fib"]),
    ("OP_Voice0", &["front_end_demo.fib"]),
    ("OP_Voice1", &["front_end_demo.fib"]),
    ("OP_Voice2", &["front_end_demo.fib"]),
    (
        "OP_Voice_Chatter",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_Voice_Commands",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "OP_Voice_Status",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    ("OverwriteGameNo", &["front_end.fib", "front_end_demo.fib"]),
    ("OverwriteGameYes", &["front_end.fib", "front_end_demo.fib"]),
    ("PLM_BreakAlliance", &["csm-all.fib"]),
    ("PLM_FormAlliance", &["csm-all.fib"]),
    ("PLM_TransferRUS", &["csm-all.fib"]),
    ("PO_CloseObjectiveWindow", &["single_player_objective.fib"]),
    ("QuitGame", &["front_end.fib", "front_end_demo.fib"]),
    ("QuitToMain", &["in_game_esc_menu.fib"]),
    (
        "RM_ClearLab",
        &["beast_research_manager.fib", "research_manager.fib"],
    ),
    (
        "RM_Construction",
        &["beast_research_manager.fib", "research_manager.fib"],
    ),
    (
        "RM_ExitMenu",
        &["beast_research_manager.fib", "research_manager.fib"],
    ),
    (
        "RM_ExtendedInfo",
        &["beast_research_manager.fib", "research_manager.fib"],
    ),
    (
        "RM_Launch",
        &["beast_research_manager.fib", "research_manager.fib"],
    ),
    (
        "RM_ResearchItem",
        &["beast_research_manager.fib", "research_manager.fib"],
    ),
    (
        "RM_TechListWindow",
        &["beast_research_manager.fib", "research_manager.fib"],
    ),
    ("SP_GameDifficulty", &["front_end.fib"]),
    ("SP_Restart_Level", &["in_game_esc_menu.fib"]),
    (
        "SelectSinglePlayerGame",
        &["front_end.fib", "front_end_demo.fib"],
    ),
    ("SinglePlayerGame", &["front_end.fib", "front_end_demo.fib"]),
    (
        "SinglePlayerOptions",
        &["front_end.fib", "front_end_demo.fib"],
    ),
    ("StartNewGame", &["multiplayer_game.fib"]),
    (
        "StartSinglePlayerGame",
        &["front_end.fib", "front_end_demo.fib"],
    ),
    (
        "TA_StartSinglePlayer",
        &["front_end.fib", "front_end_demo.fib"],
    ),
    ("TA_StartSkirmish", &["multiplayer_game.fib"]),
    (
        "TA_StartTutorial",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "multiplayer_game.fib",
        ],
    ),
    ("TB_ExitImmediately", &["taskbar.fib"]),
    ("TB_ObjectivesWindowInit", &["taskbar.fib"]),
    ("TB_SensorsManager", &["taskbar.fib"]),
    ("TM_AcceptOffer", &["trader_interface.fib"]),
    ("TM_Leave", &["trader_interface.fib"]),
    ("Tutorial1", &["front_end.fib", "front_end_demo.fib"]),
    (
        "TutorialAlertCancel",
        &["front_end.fib", "front_end_demo.fib"],
    ),
    ("TutorialSkirmishAlertCancel", &["multiplayer_game.fib"]),
    ("UTY_DontDropPlayer", &["in_game_esc_menu.fib"]),
    ("UTY_DropPlayer", &["in_game_esc_menu.fib"]),
    (
        "VO_Background",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "VO_BitmapBackground",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "VO_BitmapBullet",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "VO_BitmapDamage",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "VO_BitmapDynamicCPV",
        &["front_end.fib", "in_game_esc_menu.fib"],
    ),
    (
        "VO_BitmapFilter",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "VO_BitmapHitEffects",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "VO_BitmapInstantTransition",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "VO_BitmapLensFlare",
        &["front_end.fib", "in_game_esc_menu.fib"],
    ),
    (
        "VO_BitmapMuzzle",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "VO_BitmapRepairbots",
        &["front_end.fib", "in_game_esc_menu.fib"],
    ),
    (
        "VO_BitmapSensorsBlobs",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "VO_BitmapStipple",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "VO_Bullet",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "VO_Damage",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    ("VO_DynamicCPV", &["front_end.fib", "in_game_esc_menu.fib"]),
    (
        "VO_Filter",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "VO_Hit",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "VO_InstantTransition",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "VO_Language",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    ("VO_LensFlare", &["front_end.fib", "in_game_esc_menu.fib"]),
    (
        "VO_Muzzle",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    ("VO_Repairbots", &["front_end.fib", "in_game_esc_menu.fib"]),
    (
        "VO_SensorsBlobs",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    (
        "VO_Stipple",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
];

/// User-region draw callbacks: (name, files).
pub const DRAW_FUNCTIONS: &[(&str, &[&str])] = &[
    (
        "CM_Carrier1Draw",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    (
        "CM_Carrier2Draw",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    (
        "CM_FactoryNameDraw",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    (
        "CM_LeftArrowDraw",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    (
        "CM_MotherShipDraw",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    (
        "CM_RUs",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    (
        "CM_RightArrowDraw",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    (
        "CM_SUBarDraw",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    (
        "CM_ShipCost",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    (
        "CM_ShipNumbers",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    (
        "CM_Ships",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    (
        "CM_TotalCost",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    (
        "CM_TotalShips",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    ("CON_TextDraw", &["console.fib"]),
    (
        "CP_BaseColorPreview",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "multiplayer_game.fib",
        ],
    ),
    (
        "CP_HueSaturation",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "multiplayer_game.fib",
        ],
    ),
    (
        "CP_PreviewImage",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "multiplayer_game.fib",
        ],
    ),
    (
        "CP_PreviousDraw0",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "multiplayer_game.fib",
        ],
    ),
    (
        "CP_PreviousDraw1",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "multiplayer_game.fib",
        ],
    ),
    (
        "CP_PreviousDraw2",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "multiplayer_game.fib",
        ],
    ),
    (
        "CP_PreviousDraw3",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "multiplayer_game.fib",
        ],
    ),
    (
        "CP_PreviousDraw4",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "multiplayer_game.fib",
        ],
    ),
    (
        "CP_StripeColorPreview",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "multiplayer_game.fib",
        ],
    ),
    (
        "CP_Value",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "multiplayer_game.fib",
        ],
    ),
    ("CS_ScenarioName", &["multiplayer_game.fib"]),
    (
        "FE_DrawVersionNum",
        &["front_end.fib", "front_end_demo.fib"],
    ),
    ("HR_DrawChatBox", &["horse_race.fib"]),
    ("HR_DrawPlayersProgress", &["horse_race.fib"]),
    ("LG_CurrentChannel", &["multiplayer_lan_game.fib"]),
    ("LG_CurrentGame", &["multiplayer_lan_game.fib"]),
    ("LG_DrawMessageBox", &["multiplayer_lan_game.fib"]),
    ("LG_Protocal", &["multiplayer_lan_game.fib"]),
    (
        "LM_Carrier1Draw",
        &["beast_launch_manager.fib", "launch_manager.fib"],
    ),
    (
        "LM_Carrier2Draw",
        &["beast_launch_manager.fib", "launch_manager.fib"],
    ),
    (
        "LM_CorvetteUsed",
        &["beast_launch_manager.fib", "launch_manager.fib"],
    ),
    (
        "LM_FactoryNameDraw",
        &["beast_launch_manager.fib", "launch_manager.fib"],
    ),
    (
        "LM_FighterUsed",
        &["beast_launch_manager.fib", "launch_manager.fib"],
    ),
    ("LM_Module_Slot1", &["launch_manager.fib"]),
    ("LM_Module_Slot2", &["launch_manager.fib"]),
    ("LM_Module_Slot3", &["launch_manager.fib"]),
    ("LM_Module_Slot4", &["launch_manager.fib"]),
    ("LM_Module_Slot5", &["launch_manager.fib"]),
    ("LM_Module_Slot6", &["launch_manager.fib"]),
    ("LM_Module_Slot7", &["launch_manager.fib"]),
    (
        "LM_MotherShipDraw",
        &["beast_launch_manager.fib", "launch_manager.fib"],
    ),
    ("MG_AdBanner", &["multiplayer_game.fib"]),
    ("MG_AddCPUOpponent", &["multiplayer_game.fib"]),
    ("MG_CurrentChannel", &["multiplayer_game.fib"]),
    ("MG_CurrentGame", &["multiplayer_game.fib"]),
    ("MG_DrawMessageBox", &["multiplayer_game.fib"]),
    ("MG_DrawPatchProgress", &["multiplayer_game.fib"]),
    ("MG_NumberOfComputers", &["multiplayer_game.fib"]),
    ("MG_RemoveCPUOpponent", &["multiplayer_game.fib"]),
    ("NP_Decrease", &["front_end_demo.fib"]),
    ("NP_Draw", &["front_end_demo.fib"]),
    ("NP_Increase", &["front_end_demo.fib"]),
    (
        "OP_DrawCountdownBox",
        &["front_end.fib", "front_end_demo.fib"],
    ),
    (
        "OP_NumChannels",
        &[
            "front_end.fib",
            "front_end_demo.fib",
            "in_game_esc_menu.fib",
        ],
    ),
    ("PO_TextDraw", &["single_player_objective.fib"]),
    (
        "RM_Lab1Draw",
        &["beast_research_manager.fib", "research_manager.fib"],
    ),
    ("RM_Lab2Draw", &["research_manager.fib"]),
    ("RM_Lab3Draw", &["research_manager.fib"]),
    ("RM_Lab4Draw", &["research_manager.fib"]),
    ("RM_Lab5Draw", &["research_manager.fib"]),
    ("RM_ModuleNameDraw", &["research_manager.fib"]),
    ("RM_PussyBlob", &["beast_research_manager.fib"]),
    (
        "RM_TechImageDraw",
        &["beast_research_manager.fib", "research_manager.fib"],
    ),
    ("SM_ViewPort", &["sensors_manager.fib"]),
    (
        "SV_ShipView",
        &["beast_construction_manager.fib", "construction_manager.fib"],
    ),
    ("ShipsDockedDraw", &["hyperspace_roll_call.fib"]),
    ("ShipsRemainingDraw", &["hyperspace_roll_call.fib"]),
    ("TB_Attacking", &["taskbar.fib"]),
    ("TB_Docking", &["taskbar.fib"]),
    ("TB_Guarding", &["taskbar.fib"]),
    ("TB_Mothership", &["taskbar.fib"]),
    ("TB_Moving", &["taskbar.fib"]),
    ("TB_Other", &["taskbar.fib"]),
    ("TB_RUs", &["taskbar.fib"]),
    ("TB_Ships", &["taskbar.fib"]),
    ("TM_CostList", &["trader_interface.fib"]),
    ("TM_Dialog", &["trader_interface.fib"]),
    ("TM_TechImage", &["trader_interface.fib"]),
    ("TM_TechInfo", &["trader_interface.fib"]),
    ("TM_TechList", &["trader_interface.fib"]),
    ("TM_XRUs", &["trader_interface.fib"]),
    ("UTY_PlayerDropped", &["in_game_esc_menu.fib"]),
    (
        "dont_draw",
        &["beast_launch_manager.fib", "launch_manager.fib"],
    ),
];
