export const steps = (min, max, step) => {
  const values = [];
  for (let value = min; value <= max; value += step) values.push(value);
  return values;
};
