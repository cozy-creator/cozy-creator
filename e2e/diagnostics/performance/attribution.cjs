// A diagnostic can support matched analysis only after the actual current case positively reproduces a gate.
module.exports = red => red === true
  ? 'pending matched analysis; profiled results are not qualification'
  : 'NO regression/fix attribution: current unprofiled performance failure was not confirmed';
