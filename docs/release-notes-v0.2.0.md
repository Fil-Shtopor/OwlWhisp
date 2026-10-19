# OwlWhisp 0.2.0 preview

Custom models now live at the end of the main model list, under **Custom Models**,
alongside the existing groups of model makers.

- **Expand the action you need:** **Add local model** and **Connect online model** are
  full-width expandable rows with the same appearance as the other model rows. Each opens its
  own form. Saved local models and online connections appear in the same Custom Models group.
- **Use both forms independently:** local model names and online connection names have separate
  state. Opening or cancelling one form does not overwrite the other form's inputs.
- **Expand or collapse the complete list:** **Expand all / Collapse all** includes both custom
  model forms. The model count includes saved custom models and online connections.
- **Follow the updated documentation:** the user guide points to the new location. An Android
  and iOS architecture assessment records a proposed shared SDK and native app boundaries;
  mobile applications and mobile inference are not implemented by this release.

Custom local ONNX imports and opt-in compatible transcription APIs retain the capabilities
introduced in 0.1.9. Local model files stay in their selected folders. API keys use the operating
system credential store. Saving an online connection does not contact its provider;
**Test connection** uploads generated silence, and selecting **Use online** sends dictation
recordings to that service.

The release workflow builds and verifies all six desktop platforms, checks the complete set
of 14 release assets, and verifies public downloads against their SHA-256 checksums.

This remains an unsigned preview release. Custom local ONNX models require the sherpa engine;
the Linux ARM64 preview ships without it. macOS/Linux desktop integration and RTX Spark support
retain their existing preview limitations.
