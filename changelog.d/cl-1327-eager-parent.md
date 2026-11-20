A package's import-only executor that exits (killed, crashed, out of memory) is replaced at once, so the next request forks from a
ready one. Before, TensorD found it gone only when a request arrived, and that request waited 6–7.5 s for a new one to start and import
its package. Each import-only executor says when it is ready, in how long, and what it holds (host memory only, nothing on a GPU).
