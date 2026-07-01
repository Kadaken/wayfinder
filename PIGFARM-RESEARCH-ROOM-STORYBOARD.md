# Pig Farm Research Room Storyboard

Purpose: reusable choreography and dialogue reference for Pig Farm's animated Research Room. This should help future Codex/Claude sessions preserve the intended signal flow while sprite sheets and UI animation are built.

Core idea: characters speak as a result of their findings. Waiting animations can use neutral process text, but final speech should come from actual fetched facts and model outputs.

## Signal Flow

Scout finds candidate -> Intel builds dossier -> Bull argues upside -> Bear attacks risk -> Quant checks numbers -> Portfolio Manager decides -> Dossier exits to stock page

## Scene 1: Candidate Enters

Visual: Scout places a folder on the table. The folder becomes the ticker dossier.

Scout says:

> I found MU. It is already in our portfolio, it has recent strength, and the price is elevated. This is worth a table review.

If multiple candidates:

> I found three names worth reviewing. MU has momentum, SOFI has retail pressure, and SMCI has high volatility with risk attached.

Meaning: Scout is not just explaining why we are looking. Scout is the finder. Scout brings names worth reviewing to the table.

## Scene 2: Intel Builds the Dossier

Visual: Intel/Data Desk opens the folder. Source cards, price cards, news strips, filing cards, and unknown stamps slide into the dossier.

Intel says:

> Confirmed: we hold this position. Entry, current price, and portfolio exposure are available.

Then:

> Recent news is available. SEC filing check is available. Technical indicators are not fully loaded yet, so RSI and support levels are marked unknown.

If facts are thin:

> The briefing is thin. I am not letting anyone cite revenue, customers, short interest, RSI, or support unless it was fetched.

If strong data exists:

> We have enough facts for a first-pass debate: position data, recent price action, news, and filing context.

Meaning: Intel is the anti-hallucination role. Intel does not make the trade call. Intel defines what is known and what is unknown.

## Scene 3: Bull Makes the Upside Case

Visual: Bull stands, hands on table. Green cards appear on Bull's side of the folder.

Bull says:

> The upside case is continuation. The stock is already showing strength, and because we are already in the position, we do not need a perfect entry to benefit.

If data is weak:

> I can argue momentum, but I cannot honestly claim a catalyst yet. I need volume, news, earnings, or sector confirmation before calling this a strong buy.

If data is strong:

> The case improves because price action and fresh evidence are pointing the same way. I would add conviction if the next data point confirms continuation.

Meaning: Bull argues how the stock wins, but cannot invent supporting facts.

## Scene 4: Bear Attacks the Thesis

Visual: Bear stands across the table. Red risk cards hit the folder.

Bear says:

> The risk is chasing strength after the easy move. Elevated price without confirmed support can turn into a fast pullback.

If Bull overreaches:

> Bull is leaning on assumptions. I do not see enough fetched evidence to support that confidence.

If accounting, legal, or news risk exists:

> This is not just volatility. There is event risk here, and price action can detach from fundamentals quickly.

If risk is low:

> I do not love the entry, but I do not see a kill shot. The risk is manageable if position size stays controlled.

Meaning: Bear stress-tests the idea and calls out unsupported confidence.

## Scene 5: Quant Gives the Numbers

Visual: Quant projects numbers over the table. Position size, P/L, volatility, risk/reward, and missing metrics appear as a cyan overlay.

Quant says:

> Position size is small enough to survive normal noise, but the current evidence does not justify oversized confidence.

If indicators are missing:

> RSI, volume, ATR, and support are not given. I am not calculating a technical case from missing data.

If P/L matters:

> Current P/L says we are not deciding in a vacuum. The question is whether the remaining upside is worth the drawdown risk.

If risk is too high:

> Risk/reward is deteriorating. Either reduce exposure or require a stronger confirmation signal.

Meaning: Quant does supplied-data math only. Quant should not hallucinate indicators.

## Scene 6: Bull and Bear Clash

Visual: Bull and Bear both stand. The folder splits into green and red tabs.

Bull says:

> The position can still work if momentum continues. Cutting too early could remove us before the next leg.

Bear says:

> Momentum without proof is not a thesis. If the next move is down, we are just defending a weak entry.

If they agree:

> We agree this is not a clean buy yet.

If they disagree:

> We disagree on whether recent strength is signal or noise.

Meaning: The disagreement is visible. PM resolves it later.

## Scene 7: Portfolio Manager Listens

Visual: PM pulls Scout, Intel, Bull, Bear, and Quant cards together.

PM says:

> I have the setup, the facts, the upside case, the risk case, and the numbers.

If data is thin:

> Confidence is capped because key data is missing.

If evidence is strong:

> Confidence can rise because the briefing is sourced and the managers are aligned.

Meaning: PM is the decider. PM does not invent new facts; PM weighs the table.

## Scene 8: Final Verdict

Visual: PM stamps the folder.

For WATCH:

> Verdict: WATCH. The setup is interesting, but the evidence is not strong enough for a higher-conviction move.

For HOLD:

> Verdict: HOLD. The position is acceptable, but we need confirmation before increasing exposure.

For TRIM:

> Verdict: TRIM. The risk/reward is weakening, and the position should be reduced before momentum fades.

For AVOID:

> Verdict: AVOID. Risk dominates the upside case. Do not add exposure.

For BUY:

> Verdict: BUY. The upside case is supported by fetched facts, risk is defined, and sizing is acceptable.

Meaning: The final stamp should appear only when the actual backend verdict lands.

## Scene 9: Folder Exits to Stock Page

Visual: Completed dossier slides into the dashboard or stock page.

PM says:

> Send this to the stock page. Keep the missing facts visible. If new data arrives, reopen the room.

Intel says:

> Unknowns remain tracked. No one gets to turn missing data into confidence.

Meaning: Research output becomes visible portfolio context, not a disposable animation.

## Live Animation Rules

- Animate from backend state, not fake timers.
- If Bull takes 30 seconds, Bull stays active and speaks/process-texts for 30 seconds.
- During model generation, use neutral process text only.
- After model output returns, replace process text with actual result text.
- If a fetcher fails, Intel should mark the missing fact as unknown.
- If the model times out, the active character should show a failure or fallback state.
- If PM falls back from Claude to local model, show a local-fallback visual state.

## Safe Waiting Lines

Bull while waiting:

> Looking for the upside case...
> Testing whether momentum has support...
> Checking what would confirm continuation...
> Separating facts from assumptions...

Bear while waiting:

> Attacking the thesis...
> Checking what can go wrong...
> Looking for unsupported assumptions...
> Stress-testing downside risk...

Intel while waiting:

> Fetching quote history...
> Checking recent news...
> Looking for SEC filings...
> Marking missing facts as unknown...

Quant while waiting:

> Calculating position size...
> Checking P/L and volatility...
> Looking for supplied technicals...
> No RSI unless it was fetched.

PM while waiting:

> Listening to both sides...
> Weighing facts vs assumptions...
> Checking confidence level...
> Preparing final call...

## Signature Loop

Scout:

> I found something worth the room.

Intel:

> Here is what we actually know.

Bull:

> Here is how it wins.

Bear:

> Here is how it fails.

Quant:

> Here is what the numbers allow.

PM:

> Here is the call.

## Implementation Note

Frontend should map backend research flow state to character states:

- `scouting` -> Scout active, folder placement
- `briefing` or data-fetch phase -> Intel active, dossier filling
- `debating` with Bull -> Bull speaking loop
- `debating` with Bear -> Bear speaking loop
- `debating` with Quant -> Quant calculation loop
- `deciding` or PM phase -> Portfolio Manager thinking/stamp
- `complete` -> dossier close, verdict displayed
- `error` or stale state -> character fallback/unknown animation

The room should feel like a real research machine. The characters are not decoration; they personify the actual backend signal flow.
