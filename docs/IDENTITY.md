# Application working title and ALMA

## Application name

**Pinky is a working title only.** Its origin is not known, and it has not been
accepted as the final application name. It remains in the repository path,
package names, executable, and current interface solely to avoid a premature
technical rename while development continues.

The final application name is undecided. Choosing it should be a separate
decision followed by one controlled rename of user-facing text, package
metadata, executable and bundle identifiers, documentation, and repository
references.

## Avatar name

The reactive ASCII entity in the application's centre panel is named **ALMA**:

> **A**rchived **L**ocal **M**emory **A**ssistant

Use **ALMA** when displaying the expanded identity and **Alma** when addressing
her conversationally.

The name describes the application's core purpose:

- **Archived** — approved source versions and exact citations are retained.
- **Local** — private data and intelligence-model operations stay under the
  user's control.
- **Memory** — the vault preserves source history, task records, and eventually
  encrypted conversations.
- **Assistant** — the entity communicates the application's state and will
  present its source-grounded responses.

## Identity boundary

ALMA is the application's visible entity, not a separate process, model,
evidence source, or authority. Her animation and accessible status text must
reflect real task state. She must never imply that an operation succeeded, a
claim is supported, or a task was cancelled unless the validated runtime state
says so.

## Motion contract

The centre entity uses the same `EntityState` that drives the status pill. The
motion is intentionally state-specific rather than decorative. Its primary
silhouette remains oriented toward the viewer while it expresses a state:

| State | Visual cue |
| --- | --- |
| `idle` | A slow bounded turn, small regular breath, and soft fluid surface drift. |
| `listening` | A gentle turn toward the composer, two bright side emitters, and opposing sprite wavefronts that cross at the centre and finish at the opposite sides. |
| `thinking` | A bounded turn with aggressive contraction, turbulent deformation, and reaction-like thrashing. |
| `researching` | An expanding scan ring with rapid turbulent shape changes. |
| `creating` | Points assemble outward from a compact nucleus with surrounding sparks. |
| `waiting` | The cloud settles around three dim amber motes suspended inside it. |
| `cancelling` | The colour desaturates while the form contracts. |
| `error` | A stable red fractured form; it does not pretend to recover. |
| `completed` | A short green expansion with a spark afterglow, then a calm idle-like breath. |

When reduced motion is enabled, continuous rotation, rings, jitter, and
breathing are removed. The state remains visible through colour, scale, the
waiting-mote cue, and the live status text. Environments without WebGL use a
state-specific text silhouette instead.
