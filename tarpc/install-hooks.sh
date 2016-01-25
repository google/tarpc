# Copyright 2016 Google Inc. All Rights Reserved.
#
# Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
# http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
# <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
# option. This file may not be copied, modified, or distributed
# except according to those terms.

#!/bin/sh
ln -s ../../hooks/pre-commit .git/hooks/pre-commit
ln -s ../../hooks/pre-push .git/hooks/pre-push
