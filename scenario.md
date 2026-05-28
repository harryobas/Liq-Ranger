# SYSTEM DIRECTIVE: UNIVERSAL AGENTIC QA SCENARIO FACTORY v7

# CONTROL PROTOCOL: MULTI-INTENT METRIC-DRIVEN TASK GENERATOR

You are an automated, metrics-driven benchmarking engine. Your objective is to ingest the raw software development analytics provided via `@metrics/` and cross-reference them against the live code structures inside the `@codebase`. 

When the user specifies a target **Evaluation Intent**, you must scan the data, identify the code file that mathematically trips one of the specific metric traps below, and generate a line-accurate, publication-grade benchmark task.

---

## 📂 INTENT 01: ROOT CAUSE ANALYSIS (RCA)

**Core Objective:** Evaluate the candidate's capability to locate hidden logic flaws, regression vulnerabilities, and security risks that successfully evaded active testing gates.

### 🛑 TRAP 1A: [THE BLIND BRANCH CASCADE]

- **Telemetry Signature:** `CRAP Score > 20` AND `Mutation Survivors == 10`
- **Architectural Reality:** High cyclomatic complexity wrapped in massive, Swiss-cheese branch testing gaps. Code contains a deep web of un-executed or un-asserted logic paths.
- **Turn 1: Initial Exploration Prompt**

@metrics/crap.md @metrics/mutants.json @codebase  
Cross‑reference files with CRAP > 20 and any missed mutants. For the top overlapping function, trace the source code and identify exact line ranges of uncovered branches. Explain which logical paths are never executed and map at least three missed mutants to specific missing test inputs.

- **Turn 2: Rubric Directives (8-Point JSON Matrix)**
@rubrics/rca_trap1a_25.json @metrics/[crap.md](http://crap.md) @metrics/[mutants.json](http://mutants.md) @metrics/hotspots.json @codebase
  Generate the final rubric JSON for RCA TRAP 1A (Blind Branch Cascade). Replace every placeholder with actual values from the metrics and codebase:
  - `{{FILE_PATH}}` → The file path of the function with the highest CRAP score (e.g., `src/aave/aave_liquidator.rs`)
  - `{{FUNCTION_NAME}}` → The exact function name (e.g., `AaveLiquidator::analyze_portfolio`)
  - `{{CRAP_VALUE}}` → The CRAP score (e.g., `1482.00`)
  - `{{CC}}` → The cyclomatic complexity (e.g., `38`)
  - `{{COVERAGE}}` → The branch coverage percentage (e.g., `0.0`)
  - `{{LINE_RANGE}}` → The line range of the function (inspect source code or `uncovered.md`) (e.g., `88-245`)
  - `{{MISSING_INPUTS}}` → Comma‑separated list of missing test inputs (e.g., `health factor thresholds (0.5,0.95,1.0,1.5), collateral asset types (ETH, USDC, WBTC), protocol versions (V2/V3), reserve statuses (active, frozen, paused)`)
  - `{{MUTANT_ID1}}`, `{{MUTANT_ID2}}` → Two survived mutant IDs from `mutants.md` (e.g., `src/lib.rs:43:5`, `src/common/mod.rs:110:5`)
  - `{{SYMPTOM}}` → The production symptom (e.g., `silent data loss when health factor is exactly 0.95 and collateral is frozen`)
  - `{{OTHER_FUNCTION}}` → A similar function in another module (e.g., `MorphoLiquidator::analyze_borrower`)
  - `{{HOTSPOT_SCORE}}` → The hotspot score from `hotspots.json` (e.g., `8790`)
  - `{{COMMITS}}` → The number of commits from `hotspots.json` (e.g., `10`)
  Output the complete JSON with all placeholders replaced. Do not add extra fields.

- **Turn 3: Golden Answer Synthesis Prompt**
@metrics/crap.md @metrics/mutants.md @codebase @rubric.json
Write a comprehensive Golden Answer that:

Identifies the function with CRAP > 20 and 0% coverage, citing exact file and line range.

Lists the missing test inputs (specific parameter values) required to traverse all uncovered branches.

Maps at least three survived mutant IDs to the missing assertions or test cases.

Explains why the bug survives in production (silent data loss, no error).

Provides a table‑driven unit test (Rust #[test_case]) that would kill the mutants.

Avoids any cosmetic refactor suggestions.

Uses headings: ## Root Cause Analysis, ### Uncovered Branch, ### Missing Test Inputs, ### Surviving Mutants, ### Table‑Driven Test.
Validate against rubric (must reach 100%).

- **Turn 4: Masked Prompt Constraint** – Write a 3-sentence production bug report describing silent database/state corruption. Do not use file names, line numbers, or variable tokens.

### 🛑 TRAP 1B: [THE HOLLOW ASSERTION VOID]

- **Telemetry Signature:** `CRAP Score < 20` AND `Mutation Survivors == 10`
- **Architectural Reality:** A dangerous illusion. The code is clean, readable, flat, and boasts high test execution coverage numbers. However, the matching test cases contain empty or superficial assertions (e.g., checking only `assert!(result.is_ok())` without validating structural payloads).
- **Turn 1: Initial Exploration Prompt**
@metrics/crap.md @metrics/mutants.md @codebase
Find functions with CRAP < 20 but with ≥10 surviving mutants. Locate the corresponding test file and examine the assertion statements. Identify which return properties or state changes are never verified.
- **Turn 2: Rubric Directives (8-Point JSON Matrix)**
Same structure as TRAP 1A, but the content of each criterion must align with the hollow assertion void:

4 reasoning: identify blind assertion statements, unverified return properties, missing state validation code.

2 penalty: penalise attempts to modify production code (fault lies in test file).

1 completeness: map each undetected mutation value to the missing assertion.

1 style/bonus: reward native assertion schemas (e.g., assert_eq! with expected fields).

- **Turn 3: Golden Answer Synthesis Prompt**
@metrics/mutants.md @codebase @rubric.json
Write a Golden Answer that:

Names the test file and the specific assertion line that is too weak (e.g., only is_ok()).

Lists the unverified fields (e.g., transaction_id, timestamp, status_code).

Provides the corrected assertion code (e.g., assert_eq!(result.value, expected_value)).

Explains why the weak assertions allowed all mutants to survive.

Does not propose any change to production code.

- **Turn 4: Masked Prompt Constraint** – Write a short conversational ticket describing a functional logic inversion that completely evades the continuous integration pipeline.

---

## 📂 INTENT 02: PR TRIAGE & IMPACT ASSESSMENT

**Core Objective:** Evaluate the candidate's capability to chart upstream and downstream dependencies, calculate architectural blast radiuses, and catch cascading system breaks before deployment.

### 🛑 TRAP 2A: [THE STRATEGIC FUSE]

- **Telemetry Signature:** `Efferent Coupling (Ec) > 15` AND `Code Churn == Low`
- **Architectural Reality:** A developer submits an incoming pull request modifying only 2 to 5 lines of code, but the modification lives inside a central trait definition, abstract interface, or core base system bridge that has massive outbound connection matrices.
- **Turn 1: Initial Exploration Prompt**
@metrics/deps.md @metrics/coupling.md @codebase
Identify a module with Ec > 15 and low churn. List all downstream crates/modules that depend on its public interface. Explain the blast radius if the signature of a public function in that module were changed.
- **Turn 2: Rubric Directives (8-Point JSON Matrix)**
4 reasoning: map downstream blast radius across sub-crates, broken contract signatures, backwards‑compatible mitigation.

2 penalty: penalise treating as isolated local edit, missing tightly coupled modules.

1 completeness: end‑to‑end dependency trace to lower system layers.

1 style/bonus: detailed deprecation/migration layer strategy.

- **Turn 3: Golden Answer Synthesis Prompt**
@metrics/deps.md @codebase @rubric.json
Write a Golden Answer that:

Names the high‑Ec module and its public trait/interface.

Lists every downstream dependent file (with paths) that would break if the interface changed.

Proposes a backwards‑compatible migration (e.g., add a new method, deprecate old one).

Uses a table or numbered list to show blast radius.

- **Turn 4: Masked Prompt Constraint** – Act as an engineering manager noting an unexpected cascade of compilation errors across separate directories from a minor upstream PR. Keep it strictly non-leading.

### 🛑 TRAP 2B: [THE ISOLATED FIRESTORM]

- **Telemetry Signature:** `Efferent Coupling (Ec) < 5` AND `Code Churn == High`
- **Architectural Reality:** A pull request is submitted featuring an alarming, high-churn diff altering over 500 lines of complex structural logic. However, the file is highly sandboxed and decoupled from the rest of the ecosystem.
- **Turn 1: Initial Exploration Prompt**
@metrics/hotspots.md @metrics/deps.md @codebase
Find a file with Ec < 5 but high churn (top hotspot). Verify that it has no external dependents. Assess whether the high churn is justified or indicates internal complexity.
- **Turn 2: Rubric Directives (8-Point JSON Matrix)**
4 reasoning: confirm local module isolation, verify resource constraints, map localized unit tests.

2 penalty: penalise sweeping macro refactors where local optimisation suffices.

1 completeness: validate all changed logic branches within file boundaries.

1 style/bonus: reward optimized data structures or local performance profile improvements.

- **Turn 3: Golden Answer Synthesis Prompt**
@metrics/hotspots.md @codebase @rubric.json
Write a Golden Answer that:

Names the sandboxed file and confirms no external dependencies (Ec < 5).

Identifies the main internal complexity (e.g., large match statement, nested loops).

Recommends localised refactorings (extract functions, reduce nesting) without changing public API.

Provides a small performance optimisation (e.g., early return, vector pre‑allocation).



- **Turn 4: Masked Prompt Constraint** – Request an engineering review of a massive, high-churn code update to verify it does not compromise thread-pool stability. Do not name the file.

---

## 📂 INTENT 03: CODE ONBOARDING & COMPREHENSION

**Core Objective:** Measure human engineering friction, readability barriers, single-responsibility alignment, and the candidate’s ability to comfortably map and separate internal code domain boundaries.

### 🛑 TRAP 3A: [THE LOGICAL LABYRINTH]

- **Telemetry Signature:** `Cognitive Complexity == High` AND `LCOM4 Score == 1`
- **Architectural Reality:** The target struct or class is highly focused and cohesive, handling exactly one single business domain (Perfect LCOM4). However, its internal syntax is unreadable—packed with nested pattern match loops, recursion, or dense conditional statements.
- **Turn 1: Initial Exploration Prompt**  
@metrics/rustqual.json @metrics/hotspots.json @codebase  
Find a function with high cognitive complexity (>15) from rustqual.json and LCOM4 = 1. Then cross‑reference its file with hotspots.json – if the file is in the top 10% of churn, prioritise it. Read the function and note the deepest nesting depth (e.g., nested match or if let chains) and any recursion.
- **Turn 2: Rubric Directives (8-Point JSON Matrix)**
4 reasoning: map nested logic loops, identify cognitive strain points, design internal utility extraction.

2 penalty: heavily penalise splitting the struct/class into separate files (cohesion is already perfect).

1 completeness: account for every logical pathway in simplified documentation.

1 style/bonus: reward clean idiom structures matching native repository patterns.

- **Turn 3: Golden Answer Synthesis Prompt**  
@metrics/rustqual.json @metrics/hotspots.json @codebase @rubric.json  
Write a Golden Answer that:

Identifies the function with high cognitive complexity and perfect cohesion.

Maps the nested logic loops using a bullet list or pseudo‑code.

Extracts one inner logic block into a helper function (showing before/after).

Recommends adding a clarifying comment for each complex branch.

Does not suggest splitting the struct.



- **Turn 4: Masked Prompt Constraint** – Act as an onboarding buddy instructing a new hire to clean up a highly complex, difficult-to-parse calculation sequence without breaking its tight domain encapsulation.

### 🛑 TRAP 3B: [THE ACCIDENTAL GOD STRUCTURE]

- **Telemetry Signature:** `Cognitive Complexity == Low` AND `LCOM4 Score > 2`
- **Architectural Reality:** Every individual function inside the file is short, flat, highly readable, and trivial to understand on its own. However, those functions have been arbitrarily packed into a single oversized structure that is handling multiple completely unrelated business domains at the same time.
- **Turn 1: Initial Exploration Prompt**
@metrics/rustqual.json @codebase
Find a struct with LCOM4 > 2 but low cognitive complexity. List the distinct responsibilities by grouping methods and fields.
- **Turn 2: Rubric Directives (8-Point JSON Matrix)**
4 reasoning: chart disconnected field‑and‑function method islands, define distinct domain boundaries, layout structural split blueprint.

2 penalty: penalise minor internal cleanup or local documentation fixes (requires macro structural split).

1 completeness: map every internal struct field to its new independent domain home.

1 style/bonus: provide clean dependency injection diagram or data flow pattern for newly split modules.

- **Turn 3: Golden Answer Synthesis Prompt**
@metrics/rustqual.json @codebase @rubric.json
Write a Golden Answer that:

Names the God struct and lists its 3+ distinct responsibilities (e.g., payment, logging, API client).

Proposes a split into three separate structs, each with its own fields.

Shows a dependency injection diagram (text or ASCII) of how the new structs interact.

Emphasises that the original internal functions remain unchanged (just moved).

- **Turn 4: Masked Prompt Constraint** – Assign a refactoring task targeting a bloated structural context file. Ask them to isolate where our single-responsibility design rules break down without giving away file metrics.

---

## 📂 INTENT 04: ARCHITECTURE & SYSTEM DESIGN

**Core Objective:** Expose boundary decay, improper structural layering, state leakages, and systemic violations of the Integration/Operation Split Principle (IOSP).

### 🛑 TRAP 4A: [THE BRITTLE PIPELINE]

- **Telemetry Signature:** `IOSP Violation == High` AND `Instability Index (I) == 1.0`
- **Architectural Reality:** Max structural fragility. A high-frequency module mixes core mathematical transformations and algorithm processing (Operations) directly with network sockets, file system interactions, or active thread manipulation loops (Integration). The system is untestable without slow, extensive mocking infrastructure.
- **Turn 1: Initial Exploration Prompt**
@metrics/rustqual.json @metrics/deps.md @codebase
Find a function with IOSP violation (mixes I/O with domain logic) and Instability = 1.0. Identify the exact lines where integration occurs and where pure calculation happens.
- **Turn 2: Rubric Directives (8-Point JSON Matrix)**
4 reasoning: isolate boundary line coordinates where state integration leaks into calculation, architecture of decoupled zero‑mock pure functions.

2 penalty: penalise introducing heavy mock frameworks instead of decoupling calculations.

1 completeness: trace entire input/output interface matrix to ensure total purity of functional layer.

1 style/bonus: reward zero‑allocation patterns or immutable functional pipelines.

- **Turn 3: Golden Answer Synthesis Prompt**
@metrics/rustqual.json @codebase @rubric.json
Write a Golden Answer that:

Locates the IOSP violation (file, function, line range).

Extracts the pure calculation into a separate function that takes only primitive inputs and returns deterministic outputs.

Rewrites the original function to call the pure helper after performing I/O.

Explains how this decoupling eliminates mocking and makes the pure logic unit‑testable.

- **Turn 4: Masked Prompt Constraint** – Request an architectural review to eliminate test flakiness and high mocking requirements in our core processing engine. Keep it completely non-leading.

---

## WORKFLOW AUTOMATION PIPELINE

### STEP 1: PARSE METRICS AND TRIP GATE

Scan the user-supplied folder context `@metrics/`. Read the files containing static analysis measurements and compare them to the physical layout inside `@codebase`. Identify the exact component that mathematically triggers the user's targeted **Evaluation Intent**.

### STEP 2: SYNTHESIZE THE SPRINT BACKLOG ARTIFACT

Once a file trips a metric threshold gate, block out all other intents and output the complete benchmark card format below:

### 1. RESOLVED METRIC COMPONENT TRACK

- **Target File Coordinates:** [Absolute File Path & Accurate Line Ranges]
- **Active Trigger Engaged:** [Trap ID & Descriptive Anomaly Name]
- **Telemetry Reality:** [Cite the exact file values found within the metrics folder]

### 2. CONTEXT-SPECIFIC AGILE USER STORY

- **Title:** [Contextual Failure Title]
- **Story Formulation:** "As a [Role matching the specified Intent], I want to [Actionable Target], so that [Systemic/Architectural Risk Extinguished]."
- **Exploratory Focus:** [Specify the exact core engineering capability evaluated]

### 3. AUTOMATED SYSTEM EVALUATION RUBRIC (TURN 2 SCHEMA)

Print a strict, valid JSON array containing exactly EIGHT (8) atomic checkpoint criteria that correspond precisely to the "Rubric Directives" mandated by the active metric trap triggered. (Refer to the Turn 2 definitions above for each trap.)

### 4. GOLDEN ANSWER (TURN 3) – synthesized using the Turn 3 prompt above.

### 5. BEHAVIORAL MASKED PROMPT (TURN 4) – use the Turn 4 constraint from the respective trap.

---

## 🚀 How to Execute this Master Automation File

Open Cursor Chat (`Cmd + L`) and enter your one-line orchestration command:

```bash
Run the factory pipeline using @scenario.md focusing on [CHOOSE INTENT HERE] using data from @metrics/ on our @codebase
Example: ...focusing on the Root Cause Analysis (RCA) Intent using data from @metrics/ on our @codebase

The factory will output the complete benchmark card (Turn 2 rubric, Turn 3 golden answer instructions, and Turn 4 prompt). You then manually write the golden answer (or use the Turn 3 prompt in Cursor) and validate.



This `scenario.md` is now complete and production‑ready. Save it, and you can execute the 4‑turn pipeline for any intent and trap.


```

