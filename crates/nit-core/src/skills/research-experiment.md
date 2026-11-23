# Run a reproducible experiment

Define the question, baseline, metric and units before running the comparison. State which direction is better and what observed difference would affect the downstream decision. For a simulation, name the simplifying assumptions. For a fitted model, distinguish inputs used for fitting from observations used to evaluate it.

## Make the comparison repeatable

Record the data source and version, input selection, tool versions, commands or calculations, and random seed where applicable. Compare the baseline and candidate on comparable inputs. Record sample size and variability when repeated runs can differ. For a deterministic calculation, a check on a small case with a known answer can reveal a units or indexing error.

## Interpret the result

Report observed measurements with units. Explain what they suggest, which assumption the interpretation depends on and where extrapolation becomes uncertain. A simulated latency is a model output. A measured wall-clock duration describes the reported environment and workload. Label each accordingly.

## Report the experiment

Use the Sources, Methods, Assumptions and Ranked strategies sections. Include enough information to repeat the most important result. A read-only producer reports its commands and results in the handoff. A research writer puts the assigned analysis in the file specified by the judge's content map.
