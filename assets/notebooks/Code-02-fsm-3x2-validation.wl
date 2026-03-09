ClearAll[Code02ValidateFSM32];

Code02ValidateFSM32[] := Module[
  {
    expectedPrefix,
    expectedSuffix,
    expectedTop20,
    scoreboardRows,
    actualTop20,
    sameRowQ,
    top20Flags,
    mismatchPositions
  },
  expectedPrefix = {
    0, 1, 651, 653, 723, 725, 794, 795, 796, 797, 798, 799, 802, 807, 810,
    811, 814, 815, 818, 819, 820, 821, 822, 823, 826, 827, 828, 829, 830,
    831, 834, 835, 836, 837, 838, 839, 842, 843, 844, 845
  };
  expectedSuffix = {
    3836, 3837, 3838, 3839, 3842, 3844, 3845, 3847, 3850, 3851, 3866, 3867,
    3868, 3869, 3870, 3871, 3874, 3875, 3882, 3883
  };
  expectedTop20 = {
    <|"ID" -> 799, "Games" -> 1912, "Wins" -> 1544, "Losses" -> 164, "Ties" -> 204, "TotalPayoff" -> -1799.4, "PayoffPerGame" -> -0.9411087866108787|>,
    <|"ID" -> 823, "Games" -> 1912, "Wins" -> 1518, "Losses" -> 182, "Ties" -> 212, "TotalPayoff" -> -1815.0, "PayoffPerGame" -> -0.9492677824267782|>,
    <|"ID" -> 807, "Games" -> 1912, "Wins" -> 1544, "Losses" -> 234, "Ties" -> 134, "TotalPayoff" -> -1819.6, "PayoffPerGame" -> -0.951673640167364|>,
    <|"ID" -> 847, "Games" -> 1912, "Wins" -> 1466, "Losses" -> 236, "Ties" -> 210, "TotalPayoff" -> -1825.8, "PayoffPerGame" -> -0.9549163179916318|>,
    <|"ID" -> 2743, "Games" -> 1912, "Wins" -> 1534, "Losses" -> 182, "Ties" -> 196, "TotalPayoff" -> -1846.6, "PayoffPerGame" -> -0.9657949790794979|>,
    <|"ID" -> 1294, "Games" -> 1912, "Wins" -> 1610, "Losses" -> 96, "Ties" -> 206, "TotalPayoff" -> -1856.2, "PayoffPerGame" -> -0.97081589958159|>,
    <|"ID" -> 831, "Games" -> 1912, "Wins" -> 1476, "Losses" -> 300, "Ties" -> 136, "TotalPayoff" -> -1856.8, "PayoffPerGame" -> -0.9711297071129707|>,
    <|"ID" -> 3495, "Games" -> 1912, "Wins" -> 1514, "Losses" -> 270, "Ties" -> 128, "TotalPayoff" -> -1858.4, "PayoffPerGame" -> -0.9719665271966527|>,
    <|"ID" -> 1015, "Games" -> 1912, "Wins" -> 1550, "Losses" -> 176, "Ties" -> 186, "TotalPayoff" -> -1871.2, "PayoffPerGame" -> -0.9786610878661088|>,
    <|"ID" -> 855, "Games" -> 1912, "Wins" -> 1394, "Losses" -> 362, "Ties" -> 156, "TotalPayoff" -> -1882.8, "PayoffPerGame" -> -0.9847280334728033|>,
    <|"ID" -> 2751, "Games" -> 1912, "Wins" -> 1474, "Losses" -> 264, "Ties" -> 174, "TotalPayoff" -> -1896.4, "PayoffPerGame" -> -0.9918410041841004|>,
    <|"ID" -> 2767, "Games" -> 1912, "Wins" -> 1476, "Losses" -> 216, "Ties" -> 220, "TotalPayoff" -> -1898.2, "PayoffPerGame" -> -0.9927824267782427|>,
    <|"ID" -> 2959, "Games" -> 1912, "Wins" -> 1476, "Losses" -> 212, "Ties" -> 224, "TotalPayoff" -> -1899.2, "PayoffPerGame" -> -0.9933054393305439|>,
    <|"ID" -> 0, "Games" -> 1912, "Wins" -> 1566, "Losses" -> 0, "Ties" -> 346, "TotalPayoff" -> -1904.8, "PayoffPerGame" -> -0.996234309623431|>,
    <|"ID" -> 3279, "Games" -> 1912, "Wins" -> 1404, "Losses" -> 240, "Ties" -> 268, "TotalPayoff" -> -1915.4, "PayoffPerGame" -> -1.0017782426778243|>,
    <|"ID" -> 1023, "Games" -> 1912, "Wins" -> 1362, "Losses" -> 268, "Ties" -> 282, "TotalPayoff" -> -1916.6, "PayoffPerGame" -> -1.0024058577405858|>,
    <|"ID" -> 1246, "Games" -> 1912, "Wins" -> 1482, "Losses" -> 154, "Ties" -> 276, "TotalPayoff" -> -1924.4, "PayoffPerGame" -> -1.006485355648536|>,
    <|"ID" -> 3351, "Games" -> 1912, "Wins" -> 1556, "Losses" -> 154, "Ties" -> 202, "TotalPayoff" -> -1925.0, "PayoffPerGame" -> -1.0067991631799163|>,
    <|"ID" -> 3543, "Games" -> 1912, "Wins" -> 1522, "Losses" -> 106, "Ties" -> 284, "TotalPayoff" -> -1925.2, "PayoffPerGame" -> -1.0069037656903767|>,
    <|"ID" -> 1039, "Games" -> 1912, "Wins" -> 1476, "Losses" -> 206, "Ties" -> 230, "TotalPayoff" -> -1932.4, "PayoffPerGame" -> -1.0106694560669456|>
  };

  sameRowQ[actual_, expected_] := And[
    actual["ID"] === expected["ID"],
    actual["Games"] === expected["Games"],
    actual["Wins"] === expected["Wins"],
    actual["Losses"] === expected["Losses"],
    actual["Ties"] === expected["Ties"],
    Abs[N[actual["TotalPayoff"]] - expected["TotalPayoff"]] < 10^-12,
    Abs[N[actual["PayoffPerGame"]] - expected["PayoffPerGame"]] < 10^-12
  ];

  scoreboardRows = Normal @ TournamentScoreboard[
    {"FSM", "FSM"},
    TournamentScores[
      PerspectiveBuilders[FSMStrategyFunction, 3, 2],
      FiniteStateMachinePairs[$FSMUnique32],
      $PrisonersDilemma,
      10,
      "Parallel" -> False
    ],
    "SortKey" -> "TotalPayoff"
  ];

  actualTop20 = KeyTake[#, {"ID", "Games", "Wins", "Losses", "Ties", "TotalPayoff", "PayoffPerGame"}] & /@ Take[scoreboardRows, UpTo[20]];
  top20Flags = MapThread[sameRowQ, {actualTop20, expectedTop20}];
  mismatchPositions = Flatten @ Position[top20Flags, False];

  <|
    "FSMUnique32LengthOK" -> (Length[$FSMUnique32] === 956),
    "FSMUnique32PrefixOK" -> (Take[$FSMUnique32, Length[expectedPrefix]] === expectedPrefix),
    "FSMUnique32SuffixOK" -> (Take[$FSMUnique32, -Length[expectedSuffix]] === expectedSuffix),
    "Top20OK" -> (Length[actualTop20] === Length[expectedTop20] && And @@ top20Flags),
    "Top20MismatchPositions" -> mismatchPositions,
    "ActualTop20" -> actualTop20
  |>
]
