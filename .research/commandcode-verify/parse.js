function parseLearning(e){const{line:n}=e,o=n.match(/^-?\s*(.+?)[\.:,]?\s*confidence:\s*(\d*\.?\d+)/i);if(!o)return null;const r=parseFloat(o[2]);return isNaN(r)||r<0||r>1?null:{text:o[1].trim(),confidence:r}}
module.exports={parseLearning};
