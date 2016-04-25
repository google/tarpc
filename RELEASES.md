## 0.4 (2016-04-02)

### Breaking Changes
* Updated to the latest version of serde. Because tarpc exposes serde in
  its API, this forces downstream code to update to the latest version of
  serde, as well.

## 0.3 (2016-02-20)

### Breaking Changes
* The timeout arg to `serve` was replaced with a `Config` struct, which
  currently only contains one field, but will be expanded in the future
  to allow configuring serialization protocol, and other things.
* `serve` was changed to be a default method on the generated `Service` traits,
  and it was renamed `spawn_with_config`. A second `default fn` was added:
  `spawn`, which takes no `Config` arg.

### Other Changes
* Expanded items will no longer generate unused warnings.
