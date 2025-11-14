# CDP Update to Revision 1498597

## Date
November 14, 2025

## Summary
Updated Chrome DevTools Protocol (CDP) bindings from revision 1354347 (March 2024) to revision 1498597 (August 8, 2025). Added 18 missing `AxPropertyName` enum variants required for modern Chrome accessibility tree deserialization.

## Changes

### 1. CDP Revision Update
- **Previous**: 1354347 (March 2024)
- **Current**: 1498597 (August 8, 2025)
- **Difference**: ~144,250 Chromium commits newer
- **File**: `chromiumoxide_cdp/src/lib.rs:32`

### 2. PDL Files Updated
Downloaded fresh PDL files from ChromeDevTools/devtools-protocol repository:
- `browser_protocol.pdl` - Updated to revision 1498597
- `js_protocol.pdl` - Updated to revision 1498597

### 3. AxPropertyName Enum Enhancement
Added 18 missing variants to `AxPropertyName` enum in `browser_protocol.pdl` (lines 161-178):

```pdl
type AXPropertyName extends string
  enum
    # ... existing 43 variants ...
    url
    # LINT.IfChange(AXIgnoredReason)
    activeFullscreenElement
    activeModalDialog
    activeAriaModalDialog
    ariaHiddenElement
    ariaHiddenSubtree
    emptyAlt
    emptyText
    inertElement
    inertSubtree
    labelContainer
    labelFor
    notRendered
    notVisible
    presentationalRole
    probablyPresentational
    inactiveCarouselTabContent
    uninteresting
    # LINT.ThenChange(//third_party/blink/renderer/modules/accessibility/ax_enums.cc:AXIgnoredReason)
```

These variants represent "reasons for hiding nodes" in the accessibility tree and are returned by Chrome 115+ in the `ignoredReasons` field of accessibility nodes.

### 4. Code Regeneration
Regenerated `chromiumoxide_cdp/src/cdp.rs` (~60K lines) using the built-in test-based generator:
```bash
cargo test --test generate pdl_is_fresh          # Downloaded updated PDL files
cargo test --test generate generated_code_is_fresh  # Regenerated Rust bindings
```

All three implementations for the new enum variants were automatically generated:
- Enum definition with serde rename attributes
- `AsRef<str>` implementation for string conversion
- `FromStr` implementation with case variations

## Why This Update Was Needed

### Problem
Modern Chrome (115+) returns accessibility tree data with `AxPropertyName` variants that didn't exist in the old CDP bindings, causing serde deserialization errors:
```
Error: Serde(Error("uninteresting", line: 0, column: 0))
```

Chrome successfully generated the accessibility tree, but the Rust bindings couldn't deserialize it because the enum was missing variants.

### Root Cause
The CDP specification evolves faster than library updates. The `AxPropertyName` enum was incomplete, missing variants added to the CDP spec between March 2024 and August 2025.

## Limitations and Constraints

### PDL Format Split (August 2025)
On August 13, 2025, the CDP repository changed from a single monolithic PDL file to multiple domain-specific files with `include` directives. The chromiumoxide PDL parser doesn't currently support `include` directives.

**Impact**: This update uses revision 1498597 (August 8, 2025), which is the last revision before the format split. Newer revisions cannot be used until the parser is updated to support `include` directives.

**Timeline**:
- **Before Aug 13**: Single file format (compatible with chromiumoxide parser)
- **Aug 13+**: Split file format with `include` directives (incompatible)

### Solution Approach
Hybrid approach combining automated updates with manual patching:
1. Updated to the newest compatible CDP revision (1498597)
2. Manually added missing AxPropertyName variants from newer CDP to the PDL
3. Used automated code generation for type-safe Rust bindings

This provides the best of both worlds:
- ✅ Much newer CDP base (Aug 2025 vs Mar 2024)
- ✅ All required AxPropertyName variants
- ✅ Clean, generated code (no manual edits to cdp.rs)
- ✅ Compatible with existing chromiumoxide parser

## Testing

### Verification Steps
1. ✅ Code compiles: `cargo check --package chromiumoxide_cdp`
2. ✅ All 18 variants present in generated code (`chromiumoxide_cdp/src/cdp.rs:19663-19696`)
3. ✅ AsRef<str> implementation correct
4. ✅ FromStr implementation with case variations correct

### Real-World Testing
Tested with Chrome 142.0.7444.162 using `Accessibility.getFullAXTree` CDP command:
- ✅ Successfully deserializes accessibility trees with 16+ nodes
- ✅ Handles `ignoredReasons` containing "uninteresting" and other new variants
- ✅ No serde deserialization errors

## Future Updates

To update CDP revision again in the future:

```bash
# 1. Update revision in chromiumoxide_cdp/src/lib.rs
#    pub const CURRENT_REVISION: Revision = Revision(XXXXX);
#    Note: Use revision <= 1501221 (before Aug 13, 2025 split)

# 2. Download new PDL files
cd chromiumoxide_cdp
cargo test --test generate pdl_is_fresh

# 3. (Optional) Add any missing variants to browser_protocol.pdl
#    Check current spec at:
#    https://chromedevtools.github.io/devtools-protocol/tot/Accessibility/#type-AXPropertyName

# 4. Regenerate Rust bindings
cargo test --test generate generated_code_is_fresh

# 5. Verify compilation
cargo check --package chromiumoxide_cdp
```

## Related Links

- **CDP Specification**: https://chromedevtools.github.io/devtools-protocol/
- **CDP Repository**: https://github.com/ChromeDevTools/devtools-protocol
- **AXPropertyName Spec**: https://chromedevtools.github.io/devtools-protocol/tot/Accessibility/#type-AXPropertyName
- **Chromium Dash**: https://chromiumdash.appspot.com/ (for mapping revisions to Chrome versions)

## Files Modified

- `chromiumoxide_cdp/src/lib.rs` - Updated CURRENT_REVISION constant
- `chromiumoxide_cdp/browser_protocol.pdl` - Downloaded new version + added 18 AxPropertyName variants
- `chromiumoxide_cdp/js_protocol.pdl` - Downloaded new version
- `chromiumoxide_cdp/src/cdp.rs` - Fully regenerated from PDL files

## Attribution

This update was performed to fix compatibility with modern Chrome versions (115+) for accessibility tree operations. The approach balances staying current with the CDP specification while working within the constraints of the chromiumoxide PDL parser.
