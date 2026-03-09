ClearAll[
  Code02FSMDecodedData,
  Code02FSMOutputsFromRules,
  Code02ApplyFSMFixes
];

Code02FSMDecodedData[
  {i_Integer?NonNegative, s_Integer?Positive, k_Integer?Positive}
] := Module[
  {n = s k, max = FSMCount[s, k], t, o, nxt, out},
  If[i >= max,
    Message[FiniteStateMachineToRule::range, i, max - 1];
    Return[$Failed]
  ];
  {t, o} = QuotientRemainder[i - 1, k^s];
  nxt = If[s == 1, ConstantArray[0, n], IntegerDigits[t, s, n]];
  out = If[k == 1, ConstantArray[0, s], IntegerDigits[o, k, s]];
  <|
    "Rules" -> Thread[
      Tuples[{Range[s], Range[0, k - 1]}] ->
        Thread[{nxt + 1, out[[nxt + 1]]}]
    ],
    "Outputs" -> AssociationThread[Range[s], out]
  |>
];

Code02FSMOutputsFromRules[
  rule : {(_Rule)..},
  s_Integer?Positive,
  k_Integer?Positive
] := Module[
  {dom, rhs, out, missing},
  dom = Tuples[{Range[s], Range[0, k - 1]}];
  rhs = Lookup[Association[rule], Key /@ dom];
  out = ConstantArray[Missing["Unknown"], s];
  Scan[
    (out[[#[[1]]]] = #[[2]]) &,
    rhs
  ];
  missing = Flatten @ Position[out, _Missing];
  If[missing =!= {},
    Message[FiniteStateMachineToIndex::ambig, missing];
    Return[$Failed]
  ];
  out
];

Code02ApplyFSMFixes[] := Module[{},
  ClearAll[
    FSMStrategyFunction,
    FSMStateStepFunction,
    FiniteStateMachineToIndex
  ];

  FSMStrategyFunction[
    fsmrules_List,
    init_: 1,
    outputMap_: Automatic
  ] := Module[
    {
      actions,
      move,
      state = init,
      ruleAssoc = Association[fsmrules]
    },
    actions = Replace[
      outputMap,
      Automatic :> Association[DeleteDuplicates[Rule @@@ (Last /@ fsmrules)]]
    ];
    Function[
      {history, player},
      With[
        {last = If[history === {}, Missing["Start"], Last[history]]},
        If[
          last === Missing["Start"],
          Lookup[actions, state, 0],
          Module[
            {oi = last[[Mod[player, 2] + 1]]},
            move = Lookup[
              ruleAssoc,
              Key[{state, oi}],
              {state, Lookup[actions, state, 0]}
            ];
            state = First[move];
            Last[move]
          ]
        ]
      ]
    ]
  ];

  FSMStrategyFunction[
    fsm : {
      _Integer?NonNegative,
      _Integer?Positive,
      _Integer?Positive
    },
    init_: 1
  ] := Module[
    {decoded = Code02FSMDecodedData[fsm]},
    If[decoded === $Failed, Return[$Failed]];
    FSMStrategyFunction[decoded["Rules"], init, decoded["Outputs"]]
  ];

  FSMStateStepFunction[
    pair : {
      _Integer?NonNegative,
      _Integer?NonNegative
    },
    s_Integer?Positive,
    k_Integer?Positive
  ] := Module[
    {
      decoded1 = Code02FSMDecodedData[{pair[[1]], s, k}],
      decoded2 = Code02FSMDecodedData[{pair[[2]], s, k}],
      r1,
      r2,
      out1,
      out2
    },
    If[decoded1 === $Failed || decoded2 === $Failed, Return[$Failed]];
    r1 = Association[decoded1["Rules"]];
    r2 = Association[decoded2["Rules"]];
    out1 = decoded1["Outputs"];
    out2 = decoded2["Outputs"];
    Function[
      {q1, q2},
      {
        First @ Lookup[
          r1,
          {q1, Lookup[out2, q2, 0]},
          {q1, Lookup[out1, q1, 0]}
        ],
        First @ Lookup[
          r2,
          {q2, Lookup[out1, q1, 0]},
          {q2, Lookup[out2, q2, 0]}
        ]
      }
    ]
  ];

  FiniteStateMachineToIndex::ambig =
    "Cannot reconstruct outputs for states `1` from transition rules alone; those states never appear as transition targets.";

  FiniteStateMachineToIndex[
    rule : {(_Rule)..},
    s_Integer?Positive,
    k_Integer?Positive
  ] := Module[
    {dom, rhs, nxt, out},
    dom = Tuples[{Range[s], Range[0, k - 1]}];
    rhs = Lookup[Association[rule], Key /@ dom];
    nxt = rhs[[All, 1]] - 1;
    out = Code02FSMOutputsFromRules[rule, s, k];
    If[out === $Failed, Return[$Failed]];
    1
      + If[s == 1, 0, FromDigits[nxt, s]] * k^s
      + If[k == 1, 0, FromDigits[out, k]]
  ];

  FiniteStateMachineToIndex[
    rule : {(_Rule)..}
  ] := With[
    {lhs = First /@ rule},
    FiniteStateMachineToIndex[
      rule,
      Max[lhs[[All, 1]]],
      1 + Max[lhs[[All, 2]]]
    ]
  ];

  Null
];

Code02ApplyFSMFixes[];
