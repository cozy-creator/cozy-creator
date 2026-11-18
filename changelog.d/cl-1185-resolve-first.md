A run's model lookup no longer waits behind another run's big download: its small header and structure reads go ahead of
every run's weights, so "Finding the model" takes seconds, not the rest of a 52 GB download. A lookup still waiting after 3 s
says what it waits behind, and download progress no longer floods TensorD's log with queue lines.
