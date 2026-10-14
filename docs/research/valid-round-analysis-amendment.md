# Valid-round analysis amendment

Requested by the study operator on 2026-10-14, before scientific model generation
for the repaired repository-disjoint study. Experimental completion takes priority
over further manuscript work.

The new study's primary method comparison will use valid observations only.
No-output, provider-quota/balance interruption, exhausted token budget, timeout,
invalid model response, and runtime/infrastructure failure are excluded from that
comparison. A usable, externally graded but incorrect patch remains a valid
observation; validity is not correctness or successful delivery. An empty patch
is not by itself evidence of a missing model response: an explicit completed
no-change decision can still be graded and must not be excluded merely because
it is incorrect. A deliberate review hold with usable output is distinct from
an execution failure.

Paired B/C comparisons require both observations in the same task/variant pair
to meet the prospectively frozen validity criteria. Record excluded cells,
exclusion reasons, arm-specific validity rates, and retained task/repository
counts separately. This conditions the estimand on evaluable pairs, and differential
exclusion must be disclosed rather than interpreted as method efficacy.

Historical freezes, original rows, and original analyses remain immutable. Failed
old runs are audit and operational evidence only, not pooled with the repaired
study's efficacy results. Any historical valid-only reanalysis is a separately
labelled operator-requested post-hoc sensitivity analysis, not the original
preregistered result. Freeze the repaired study's executable eligibility rules
and analysis plan before its first scientific model call.
